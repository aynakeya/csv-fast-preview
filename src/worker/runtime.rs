use crate::core::{CsvIndex, FilterMode};
use crossbeam_channel::{Receiver, Sender};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use super::export::export_rows;
use super::message::{Event, Job};
use super::row_cache::RowCache;
use super::snapshot::CsvSnapshot;

type SharedIndex = Arc<Mutex<Option<CsvIndex>>>;
const ROW_READ_CHUNK_SIZE: usize = 128;

pub(super) fn run_worker(
    job_rx: Receiver<Job>,
    evt_tx: Sender<Event>,
    cancel_filter: Arc<AtomicBool>,
    cancel_search: Arc<AtomicBool>,
) {
    let current: SharedIndex = Arc::new(Mutex::new(None));
    let open_epoch = Arc::new(AtomicU64::new(0));
    let mut row_cache = RowCache::new();
    let mut pending_jobs = VecDeque::new();

    loop {
        let job = match pending_jobs.pop_front() {
            Some(job) => job,
            None => match job_rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            },
        };
        match job {
            Job::OpenFile { path, config } => {
                row_cache.clear();
                let epoch = open_epoch.fetch_add(1, Ordering::Relaxed) + 1;

                let preview =
                    CsvIndex::preview(&path, config.clone(), 200).map_err(|e| e.to_string());
                if let Ok(index) = &preview {
                    *current.lock().expect("current index lock") = Some(index.clone());
                }
                let _ = evt_tx.send(Event::Previewed(
                    preview
                        .as_ref()
                        .map(CsvSnapshot::from)
                        .map_err(Clone::clone),
                ));

                let total_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let started = preview
                    .as_ref()
                    .map(|index| (CsvSnapshot::from(index), total_bytes))
                    .map_err(Clone::clone);
                let _ = evt_tx.send(Event::IndexStarted(started));

                spawn_indexer(
                    epoch,
                    open_epoch.clone(),
                    current.clone(),
                    evt_tx.clone(),
                    path,
                    config,
                );
            }
            Job::Filter { col, keyword, mode } => {
                handle_filter(&current, &evt_tx, &cancel_filter, col, keyword, mode);
            }
            Job::Search { keyword } => {
                handle_search(&current, &evt_tx, &cancel_search, keyword);
            }
            Job::ReadRows { request_id, rows } => {
                let (request_id, rows) =
                    latest_read_rows_job(request_id, rows, &mut pending_jobs, &job_rx);
                handle_read_rows(
                    &current,
                    &evt_tx,
                    &mut row_cache,
                    &mut pending_jobs,
                    &job_rx,
                    request_id,
                    rows,
                );
            }
            Job::ExportRows {
                path,
                rows,
                visible_columns,
            } => {
                handle_export_rows(&current, &evt_tx, path, rows, visible_columns);
            }
        }
    }
}

fn latest_read_rows_job(
    request_id: u64,
    rows: Vec<(usize, usize)>,
    pending_jobs: &mut VecDeque<Job>,
    job_rx: &Receiver<Job>,
) -> (u64, Vec<(usize, usize)>) {
    let mut latest = (request_id, rows);

    while matches!(pending_jobs.front(), Some(Job::ReadRows { .. })) {
        let Some(Job::ReadRows { request_id, rows }) = pending_jobs.pop_front() else {
            unreachable!();
        };
        latest = (request_id, rows);
    }

    while let Ok(job) = job_rx.try_recv() {
        match job {
            Job::ReadRows { request_id, rows } => {
                latest = (request_id, rows);
            }
            other => {
                pending_jobs.push_back(other);
                break;
            }
        }
    }

    latest
}

fn interrupting_read_rows_job(
    pending_jobs: &mut VecDeque<Job>,
    job_rx: &Receiver<Job>,
) -> Option<Job> {
    let mut latest = None;

    while matches!(pending_jobs.front(), Some(Job::ReadRows { .. })) {
        let Some(Job::ReadRows { request_id, rows }) = pending_jobs.pop_front() else {
            unreachable!();
        };
        latest = Some((request_id, rows));
    }

    while let Ok(job) = job_rx.try_recv() {
        match job {
            Job::ReadRows { request_id, rows } => {
                latest = Some((request_id, rows));
            }
            other => pending_jobs.push_back(other),
        }
    }

    latest.map(|(request_id, rows)| Job::ReadRows { request_id, rows })
}

fn spawn_indexer(
    epoch: u64,
    open_epoch: Arc<AtomicU64>,
    current: SharedIndex,
    evt_tx: Sender<Event>,
    path: String,
    config: crate::core::CsvConfig,
) {
    thread::spawn(move || {
        let tx = evt_tx.clone();
        let current_for_progress = current.clone();
        let open_epoch_for_progress = open_epoch.clone();
        let result = CsvIndex::build_with_progress(
            path,
            config,
            move |offsets, indexed_rows, bytes, total_bytes| {
                if open_epoch_for_progress.load(Ordering::Relaxed) != epoch {
                    return;
                }
                if let Some(index) = current_for_progress
                    .lock()
                    .expect("current index lock")
                    .as_mut()
                {
                    if indexed_rows == offsets.len() {
                        index.row_offsets = offsets;
                    } else {
                        index.row_offsets.extend(offsets);
                    }
                }
                let _ = tx.send(Event::IndexProgress {
                    indexed_rows,
                    bytes,
                    total_bytes,
                });
            },
        )
        .map_err(|e| e.to_string());

        if open_epoch.load(Ordering::Relaxed) != epoch {
            return;
        }
        if let Ok(index) = &result {
            *current.lock().expect("current index lock") = Some(index.clone());
        }
        let _ = evt_tx.send(Event::Opened(
            result.as_ref().map(CsvSnapshot::from).map_err(Clone::clone),
        ));
    });
}

fn handle_filter(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    cancel_filter: &Arc<AtomicBool>,
    col: usize,
    keyword: String,
    mode: FilterMode,
) {
    let Some(index) = current.lock().expect("current index lock").clone() else {
        let _ = evt_tx.send(Event::Filtered(Err("No CSV opened".to_string())));
        return;
    };
    cancel_filter.store(false, Ordering::Relaxed);
    let cancelled = Arc::clone(cancel_filter);
    let tx = evt_tx.clone();
    let result = index
        .filter_rows_with_progress(col, &keyword, mode, move |done, total| {
            let _ = tx.send(Event::FilterProgress { done, total });
            cancelled.load(Ordering::Relaxed)
        })
        .map_err(|e| e.to_string());
    if cancel_filter.load(Ordering::Relaxed) {
        let _ = evt_tx.send(Event::FilterCancelled);
    } else {
        let _ = evt_tx.send(Event::Filtered(result));
    }
}

fn handle_search(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    cancel_search: &Arc<AtomicBool>,
    keyword: String,
) {
    let Some(index) = current.lock().expect("current index lock").clone() else {
        let _ = evt_tx.send(Event::Searched(Err("No CSV opened".to_string())));
        return;
    };
    cancel_search.store(false, Ordering::Relaxed);
    let cancelled = Arc::clone(cancel_search);
    let tx = evt_tx.clone();
    let result = index
        .search_rows_with_progress(&keyword, move |done, total| {
            let _ = tx.send(Event::SearchProgress { done, total });
            cancelled.load(Ordering::Relaxed)
        })
        .map_err(|e| e.to_string());
    if cancel_search.load(Ordering::Relaxed) {
        let _ = evt_tx.send(Event::SearchCancelled);
    } else {
        let _ = evt_tx.send(Event::Searched(result));
    }
}

fn handle_read_rows(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    row_cache: &mut RowCache,
    pending_jobs: &mut VecDeque<Job>,
    job_rx: &Receiver<Job>,
    request_id: u64,
    rows: Vec<(usize, usize)>,
) {
    let Some(index) = current.lock().expect("current index lock").clone() else {
        let _ = evt_tx.send(Event::RowsRead {
            request_id,
            rows: Vec::new(),
        });
        return;
    };

    let mut cached = Vec::new();
    let mut missing_logical = Vec::new();
    let mut missing_real = Vec::new();
    for (logical_idx, real_row) in rows.iter().copied() {
        if let Some(row) = row_cache.get(real_row) {
            cached.push((logical_idx, row.clone()));
        } else {
            missing_logical.push(logical_idx);
            missing_real.push(real_row);
        }
    }

    send_rows_read(evt_tx, request_id, &mut cached);

    if missing_real.is_empty() {
        return;
    }

    for (logical_chunk, real_chunk) in missing_logical
        .chunks(ROW_READ_CHUNK_SIZE)
        .zip(missing_real.chunks(ROW_READ_CHUNK_SIZE))
    {
        let mut loaded = Vec::new();
        if let Ok(rows) = index.read_page(real_chunk, 0, real_chunk.len()) {
            for ((logical_idx, real_row), row) in logical_chunk
                .iter()
                .copied()
                .zip(real_chunk.iter().copied())
                .zip(rows.into_iter())
            {
                row_cache.insert(real_row, row.clone());
                loaded.push((logical_idx, row));
            }
        }
        send_rows_read(evt_tx, request_id, &mut loaded);
        if let Some(job) = interrupting_read_rows_job(pending_jobs, job_rx) {
            pending_jobs.push_front(job);
            return;
        }
    }
}

fn send_rows_read(evt_tx: &Sender<Event>, request_id: u64, rows: &mut Vec<(usize, Vec<String>)>) {
    if rows.is_empty() {
        return;
    }
    rows.sort_by_key(|(logical_idx, _)| *logical_idx);
    let _ = evt_tx.send(Event::RowsRead {
        request_id,
        rows: std::mem::take(rows),
    });
}

fn handle_export_rows(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    path: String,
    rows: Vec<usize>,
    visible_columns: Vec<usize>,
) {
    let Some(index) = current.lock().expect("current index lock").clone() else {
        let _ = evt_tx.send(Event::Exported(Err("No CSV opened".to_string())));
        return;
    };
    let result = export_rows(&index, &path, &rows, &visible_columns)
        .map(|()| path)
        .map_err(|e| e.to_string());
    let _ = evt_tx.send(Event::Exported(result));
}
