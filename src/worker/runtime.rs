use crate::core::{CsvIndex, FilterMode};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use super::export::export_rows;
use super::message::{Event, Job};
use super::row_cache::RowCache;
use super::snapshot::CsvSnapshot;

type SharedIndex = Arc<Mutex<Option<CsvIndex>>>;

pub(super) fn run_worker(
    job_rx: Receiver<Job>,
    evt_tx: Sender<Event>,
    cancel_filter: Arc<AtomicBool>,
    cancel_search: Arc<AtomicBool>,
) {
    let current: SharedIndex = Arc::new(Mutex::new(None));
    let open_epoch = Arc::new(AtomicU64::new(0));
    let mut row_cache = RowCache::new();

    while let Ok(job) = job_rx.recv() {
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
                let started = CsvIndex::preview(&path, config.clone(), 1)
                    .map(|mut index| {
                        index.row_offsets.clear();
                        *current.lock().expect("current index lock") = Some(index.clone());
                        (CsvSnapshot::from(&index), total_bytes)
                    })
                    .map_err(|e| e.to_string());
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
            Job::ReadRows {
                request_id,
                start,
                rows,
            } => {
                handle_read_rows(&current, &evt_tx, &mut row_cache, request_id, start, rows);
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
                    index.row_offsets.extend(offsets);
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
    request_id: u64,
    start: usize,
    rows: Vec<usize>,
) {
    let Some(index) = current.lock().expect("current index lock").clone() else {
        let _ = evt_tx.send(Event::RowsRead {
            request_id,
            start,
            rows: Vec::new(),
        });
        return;
    };

    let mut out = Vec::new();
    let mut missing = Vec::new();
    let mut missing_positions = Vec::new();
    for (offset, real_row) in rows.iter().copied().enumerate() {
        if let Some(row) = row_cache.get(real_row) {
            out.push((start + offset, row.clone()));
        } else {
            missing.push(real_row);
            missing_positions.push(start + offset);
        }
    }

    if !missing.is_empty() {
        if let Ok(loaded) = index.read_page(&missing, 0, missing.len()) {
            for ((logical_idx, real_row), row) in missing_positions
                .into_iter()
                .zip(missing.into_iter())
                .zip(loaded.into_iter())
            {
                row_cache.insert(real_row, row.clone());
                out.push((logical_idx, row));
            }
        }
    }

    out.sort_by_key(|(logical_idx, _)| *logical_idx);
    let _ = evt_tx.send(Event::RowsRead {
        request_id,
        start,
        rows: out,
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
