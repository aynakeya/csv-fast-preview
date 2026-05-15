use crate::core::{CsvIndex, CsvReadPlan, UniqueColumnIndex};
use crossbeam_channel::{Receiver, Sender};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use super::export::export_rows;
use super::message::{Event, FilterRows, Job, UniqueFilter};
use super::snapshot::CsvSnapshot;

type SharedIndex = Arc<Mutex<Option<Arc<RwLock<CsvIndex>>>>>;
type SharedUniqueIndexes = Arc<Mutex<HashMap<usize, Arc<UniqueColumnIndex>>>>;
const ROW_READ_CHUNK_SIZE: usize = 32;

struct ReadRowsRequest {
    request_id: u64,
    rows: Vec<(usize, usize)>,
    columns: Vec<usize>,
}

struct CachedReadFile {
    path: PathBuf,
    file: File,
}

pub(super) fn run_worker(
    job_rx: Receiver<Job>,
    evt_tx: Sender<Event>,
    cancel_filter: Arc<AtomicBool>,
) {
    let current: SharedIndex = Arc::new(Mutex::new(None));
    let unique_indexes: SharedUniqueIndexes = Arc::new(Mutex::new(HashMap::new()));
    let open_epoch = Arc::new(AtomicU64::new(0));
    let mut pending_jobs = VecDeque::new();
    let mut read_file = None;

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
                read_file = None;
                unique_indexes.lock().expect("unique index lock").clear();
                let epoch = open_epoch.fetch_add(1, Ordering::Relaxed) + 1;

                let preview =
                    CsvIndex::preview(&path, config.clone(), 200).map_err(|e| e.to_string());
                if let Ok(index) = &preview {
                    *current.lock().expect("current index lock") =
                        Some(Arc::new(RwLock::new(index.clone())));
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
            Job::IndexUnique { col } => {
                handle_unique_index(&current, &unique_indexes, &evt_tx, &cancel_filter, col);
            }
            Job::ApplyUniqueFilters { filters } => {
                handle_unique_filter(&current, &unique_indexes, &evt_tx, &cancel_filter, filters);
            }
            Job::ReadRows {
                request_id,
                rows,
                columns,
            } => {
                let (request_id, rows, columns) =
                    latest_read_rows_job(request_id, rows, columns, &mut pending_jobs, &job_rx);
                handle_read_rows(
                    &current,
                    &evt_tx,
                    &mut read_file,
                    &mut pending_jobs,
                    &job_rx,
                    ReadRowsRequest {
                        request_id,
                        rows,
                        columns,
                    },
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
    columns: Vec<usize>,
    pending_jobs: &mut VecDeque<Job>,
    job_rx: &Receiver<Job>,
) -> (u64, Vec<(usize, usize)>, Vec<usize>) {
    let mut latest = (request_id, rows, columns);

    while matches!(pending_jobs.front(), Some(Job::ReadRows { .. })) {
        let Some(Job::ReadRows {
            request_id,
            rows,
            columns,
        }) = pending_jobs.pop_front()
        else {
            unreachable!();
        };
        latest = (request_id, rows, columns);
    }

    while let Ok(job) = job_rx.try_recv() {
        match job {
            Job::ReadRows {
                request_id,
                rows,
                columns,
            } => {
                latest = (request_id, rows, columns);
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
        let Some(Job::ReadRows {
            request_id,
            rows,
            columns,
        }) = pending_jobs.pop_front()
        else {
            unreachable!();
        };
        latest = Some((request_id, rows, columns));
    }

    while let Ok(job) = job_rx.try_recv() {
        match job {
            Job::ReadRows {
                request_id,
                rows,
                columns,
            } => {
                latest = Some((request_id, rows, columns));
            }
            other => pending_jobs.push_back(other),
        }
    }

    latest.map(|(request_id, rows, columns)| Job::ReadRows {
        request_id,
        rows,
        columns,
    })
}

fn current_index(current: &SharedIndex) -> Option<Arc<RwLock<CsvIndex>>> {
    current.lock().expect("current index lock").clone()
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
                    .as_ref()
                {
                    let mut index = index.write().expect("current index write lock");
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
        match result {
            Ok(index) => {
                let snapshot = CsvSnapshot::from(&index);
                *current.lock().expect("current index lock") = Some(Arc::new(RwLock::new(index)));
                let _ = evt_tx.send(Event::Opened(Ok(snapshot)));
            }
            Err(err) => {
                let _ = evt_tx.send(Event::Opened(Err(err)));
            }
        }
    });
}

fn handle_unique_index(
    current: &SharedIndex,
    unique_indexes: &SharedUniqueIndexes,
    evt_tx: &Sender<Event>,
    cancel_filter: &Arc<AtomicBool>,
    col: usize,
) {
    let Some(index) = current_index(current) else {
        let _ = evt_tx.send(Event::UniqueIndexed {
            col,
            result: Err("No CSV opened".to_string()),
        });
        return;
    };
    cancel_filter.store(false, Ordering::Relaxed);
    let cancelled = Arc::clone(cancel_filter);
    let done_flag = Arc::clone(cancel_filter);
    let evt_tx = evt_tx.clone();
    let unique_indexes = Arc::clone(unique_indexes);
    thread::spawn(move || {
        let tx = evt_tx.clone();
        let (path, config, total_rows) = {
            let index = index.read().expect("current index read lock");
            (
                index.path.clone(),
                index.config.clone(),
                index.row_offsets.len(),
            )
        };
        let result = CsvIndex::index_unique_values_for_file(
            &path,
            &config,
            total_rows,
            col,
            move |done, total| {
                let _ = tx.send(Event::UniqueIndexProgress { col, done, total });
                cancelled.load(Ordering::Relaxed)
            },
        )
        .map_err(|e| e.to_string());
        if done_flag.load(Ordering::Relaxed) {
            let _ = evt_tx.send(Event::FilterCancelled);
        } else {
            let event_result = match result {
                Ok(index) => {
                    let values = index.values.clone();
                    unique_indexes
                        .lock()
                        .expect("unique index lock")
                        .insert(col, Arc::new(index));
                    Ok(values)
                }
                Err(err) => Err(err),
            };
            let _ = evt_tx.send(Event::UniqueIndexed {
                col,
                result: event_result,
            });
        }
    });
}

fn handle_unique_filter(
    current: &SharedIndex,
    unique_indexes: &SharedUniqueIndexes,
    evt_tx: &Sender<Event>,
    cancel_filter: &Arc<AtomicBool>,
    filters: HashMap<usize, UniqueFilter>,
) {
    let Some(index) = current_index(current) else {
        let _ = evt_tx.send(Event::Filtered(Err("No CSV opened".to_string())));
        return;
    };
    cancel_filter.store(false, Ordering::Relaxed);
    let cancelled = Arc::clone(cancel_filter);
    let done_flag = Arc::clone(cancel_filter);
    let evt_tx = evt_tx.clone();
    let total_rows = index
        .read()
        .expect("current index read lock")
        .row_offsets
        .len();
    let unique_indexes = unique_indexes.lock().expect("unique index lock").clone();
    thread::spawn(move || {
        let result =
            filter_from_unique_indexes(total_rows, &unique_indexes, &filters, |done, total| {
                let _ = evt_tx.send(Event::FilterProgress { done, total });
                cancelled.load(Ordering::Relaxed)
            });
        if done_flag.load(Ordering::Relaxed) {
            let _ = evt_tx.send(Event::FilterCancelled);
        } else {
            let _ = evt_tx.send(Event::Filtered(result));
        }
    });
}

fn filter_from_unique_indexes<F>(
    total_rows: usize,
    unique_indexes: &HashMap<usize, Arc<UniqueColumnIndex>>,
    filters: &HashMap<usize, UniqueFilter>,
    mut on_progress: F,
) -> Result<FilterRows, String>
where
    F: FnMut(usize, usize) -> bool,
{
    let mut active: Vec<(usize, &UniqueFilter)> = filters
        .iter()
        .filter_map(|(col, filter)| match filter {
            UniqueFilter::Include(selected) if selected.is_empty() => None,
            UniqueFilter::Exclude(excluded) if excluded.is_empty() => None,
            _ => Some((*col, filter)),
        })
        .collect();
    if active.is_empty() {
        return Ok(FilterRows::All(total_rows));
    }
    active.sort_by_key(|(col, filter)| {
        unique_indexes
            .get(col)
            .map(|unique_index| estimated_rows_for_filter(unique_index, filter))
            .unwrap_or(usize::MAX)
    });

    let total = active.len();
    let mut selection = RowSelection::All(total_rows);
    for (idx, (col, filter)) in active.iter().enumerate() {
        let Some(unique_index) = unique_indexes.get(col) else {
            return Err(format!("Column {col} unique values are not indexed"));
        };
        selection = selection.intersect(rows_for_filter(unique_index, filter));
        if on_progress(idx + 1, total) {
            return Ok(FilterRows::Rows(Vec::new()));
        }
    }

    Ok(selection.into_filter_rows())
}

enum RowSelection {
    All(usize),
    AllExcept { total: usize, excluded: Vec<usize> },
    Rows { total: usize, rows: Vec<usize> },
}

impl RowSelection {
    fn intersect(self, other: RowSelection) -> RowSelection {
        match (self, other) {
            (Self::All(_), right) => right,
            (left, Self::All(_)) => left,
            (Self::Rows { total, rows: left }, Self::Rows { rows: right, .. }) => Self::Rows {
                total,
                rows: intersect_sorted_rows(&left, &right),
            },
            (Self::Rows { total, rows }, Self::AllExcept { excluded, .. })
            | (Self::AllExcept { total, excluded }, Self::Rows { rows, .. }) => Self::Rows {
                total,
                rows: subtract_sorted_rows(&rows, &excluded),
            },
            (
                Self::AllExcept {
                    total,
                    excluded: left,
                },
                Self::AllExcept {
                    excluded: right, ..
                },
            ) => Self::AllExcept {
                total,
                excluded: union_sorted_rows(&left, &right),
            },
        }
    }

    fn into_filter_rows(self) -> FilterRows {
        match self {
            Self::All(total) => FilterRows::All(total),
            Self::AllExcept { total, excluded } => compact_exclusion(total, excluded),
            Self::Rows { total, rows } => compact_rows(total, rows),
        }
    }
}

fn compact_rows(total: usize, rows: Vec<usize>) -> FilterRows {
    if rows.is_empty() {
        return FilterRows::Rows(rows);
    }
    let excluded_len = total.saturating_sub(rows.len());
    if excluded_len < rows.len() {
        FilterRows::AllExcept {
            total,
            excluded: complement_from_selected_rows(total, &rows),
        }
    } else {
        FilterRows::Rows(rows)
    }
}

fn compact_exclusion(total: usize, excluded: Vec<usize>) -> FilterRows {
    if excluded.is_empty() {
        return FilterRows::All(total);
    }
    if excluded.len() <= total.saturating_sub(excluded.len()) {
        return FilterRows::AllExcept { total, excluded };
    }
    FilterRows::Rows(complement_sorted_rows(total, &excluded))
}

fn rows_for_filter(index: &UniqueColumnIndex, filter: &UniqueFilter) -> RowSelection {
    match filter {
        UniqueFilter::Include(selected) => {
            let selected_rows = count_rows_for_selected_values(index, selected);
            let total = index.total_rows();
            if total.saturating_sub(selected_rows) < selected_rows {
                let (excluded_indices, excluded_rows) =
                    inverse_value_indices(index, selected, selected_rows);
                RowSelection::AllExcept {
                    total,
                    excluded: rows_for_value_indices(index, excluded_indices, excluded_rows),
                }
            } else {
                let selected_indices = selected_value_indices(index, selected);
                RowSelection::Rows {
                    total,
                    rows: rows_for_value_indices(index, selected_indices, selected_rows),
                }
            }
        }
        UniqueFilter::Exclude(excluded) => {
            let excluded_rows = count_rows_for_selected_values(index, excluded);
            let total = index.total_rows();
            if total.saturating_sub(excluded_rows) < excluded_rows {
                let (included_indices, included_rows) =
                    inverse_value_indices(index, excluded, excluded_rows);
                RowSelection::Rows {
                    total,
                    rows: rows_for_value_indices(index, included_indices, included_rows),
                }
            } else {
                let excluded_indices = selected_value_indices(index, excluded);
                RowSelection::AllExcept {
                    total,
                    excluded: rows_for_value_indices(index, excluded_indices, excluded_rows),
                }
            }
        }
    }
}

fn estimated_rows_for_filter(index: &UniqueColumnIndex, filter: &UniqueFilter) -> usize {
    match filter {
        UniqueFilter::Include(selected) => count_rows_for_selected_values(index, selected),
        UniqueFilter::Exclude(excluded) => index
            .total_rows()
            .saturating_sub(count_rows_for_selected_values(index, excluded)),
    }
}

fn selected_value_indices(index: &UniqueColumnIndex, selected: &HashSet<String>) -> Vec<usize> {
    selected
        .iter()
        .filter_map(|value| {
            index
                .values
                .binary_search_by(|item| item.value.as_str().cmp(value.as_str()))
                .ok()
        })
        .collect()
}

fn inverse_value_indices(
    index: &UniqueColumnIndex,
    selected: &HashSet<String>,
    selected_rows: usize,
) -> (Vec<usize>, usize) {
    let indices = index
        .values
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| (!selected.contains(item.value.as_str())).then_some(idx))
        .collect();
    (indices, index.total_rows().saturating_sub(selected_rows))
}

fn count_rows_for_selected_values(index: &UniqueColumnIndex, selected: &HashSet<String>) -> usize {
    selected
        .iter()
        .filter_map(|value| {
            index
                .values
                .binary_search_by(|item| item.value.as_str().cmp(value.as_str()))
                .ok()
                .map(|value_idx| index.values[value_idx].count)
        })
        .sum()
}

fn rows_for_value_indices(
    index: &UniqueColumnIndex,
    value_indices: Vec<usize>,
    total_rows: usize,
) -> Vec<usize> {
    let mut rows = Vec::with_capacity(total_rows);
    for value_idx in value_indices {
        rows.extend(index.rows_for_value_index(value_idx));
    }
    rows.sort_unstable();
    rows.dedup();
    rows
}

fn intersect_sorted_rows(left: &[usize], right: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(left.len().min(right.len()));
    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(left[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

fn subtract_sorted_rows(rows: &[usize], excluded: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(rows.len());
    let mut j = 0;
    for &row in rows {
        while j < excluded.len() && excluded[j] < row {
            j += 1;
        }
        if j == excluded.len() || excluded[j] != row {
            out.push(row);
        }
    }
    out
}

fn union_sorted_rows(left: &[usize], right: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => {
                out.push(left[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(right[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push(left[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    out
}

fn complement_sorted_rows(total: usize, excluded: &[usize]) -> Vec<usize> {
    let mut rows = Vec::with_capacity(total.saturating_sub(excluded.len()));
    let mut excluded_idx = 0;
    for row in 0..total {
        if excluded_idx < excluded.len() && excluded[excluded_idx] == row {
            excluded_idx += 1;
        } else {
            rows.push(row);
        }
    }
    rows
}

fn complement_from_selected_rows(total: usize, selected: &[usize]) -> Vec<usize> {
    let mut rows = Vec::with_capacity(total.saturating_sub(selected.len()));
    let mut selected_idx = 0;
    for row in 0..total {
        if selected_idx < selected.len() && selected[selected_idx] == row {
            selected_idx += 1;
        } else {
            rows.push(row);
        }
    }
    rows
}

fn handle_read_rows(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    read_file: &mut Option<CachedReadFile>,
    pending_jobs: &mut VecDeque<Job>,
    job_rx: &Receiver<Job>,
    request: ReadRowsRequest,
) {
    let ReadRowsRequest {
        request_id,
        rows,
        columns,
    } = request;

    let Some(index) = current_index(current) else {
        let _ = evt_tx.send(Event::RowsRead {
            request_id,
            rows: Vec::new(),
        });
        send_rows_read_done(evt_tx, request_id);
        return;
    };

    if columns.is_empty() {
        send_rows_read_done(evt_tx, request_id);
        return;
    }

    if rows.is_empty() {
        send_rows_read_done(evt_tx, request_id);
        return;
    }

    let mut missing_rows = rows;
    if missing_rows.len() > ROW_READ_CHUNK_SIZE {
        missing_rows[ROW_READ_CHUNK_SIZE..].sort_by_key(|(_, real_row)| *real_row);
    }

    let real_rows: Vec<usize> = missing_rows.iter().map(|(_, real_row)| *real_row).collect();
    let read_plan = index
        .read()
        .expect("current index read lock")
        .read_plan(&real_rows);
    let Ok(file) = cached_read_file(read_file, &read_plan) else {
        send_rows_read_done(evt_tx, request_id);
        return;
    };

    let mut plan_start = 0usize;
    for chunk in missing_rows.chunks(ROW_READ_CHUNK_SIZE) {
        let mut loaded = Vec::new();
        let plan_end = plan_start + chunk.len();
        if let Ok(rows) =
            read_plan.read_columns_range_with_file(file, &columns, plan_start, plan_end)
        {
            for (logical_idx, cells) in chunk.iter().map(|(logical_idx, _)| *logical_idx).zip(rows)
            {
                loaded.push((logical_idx, cells));
            }
        }
        plan_start = plan_end;
        send_rows_read(evt_tx, request_id, &mut loaded);
        if let Some(job) = interrupting_read_rows_job(pending_jobs, job_rx) {
            pending_jobs.push_front(job);
            send_rows_read_done(evt_tx, request_id);
            return;
        }
    }
    send_rows_read_done(evt_tx, request_id);
}

fn cached_read_file<'a>(
    cache: &'a mut Option<CachedReadFile>,
    read_plan: &CsvReadPlan,
) -> anyhow::Result<&'a mut File> {
    let path = read_plan.path();
    let needs_open = cache
        .as_ref()
        .is_none_or(|cached| cached.path.as_path() != path);
    if needs_open {
        *cache = Some(CachedReadFile {
            path: path.to_path_buf(),
            file: read_plan.open_file()?,
        });
    }

    Ok(&mut cache.as_mut().expect("cached read file").file)
}

fn send_rows_read(
    evt_tx: &Sender<Event>,
    request_id: u64,
    rows: &mut Vec<(usize, Vec<(usize, String)>)>,
) {
    if rows.is_empty() {
        return;
    }
    rows.sort_by_key(|(logical_idx, _)| *logical_idx);
    let _ = evt_tx.send(Event::RowsRead {
        request_id,
        rows: std::mem::take(rows),
    });
}

fn send_rows_read_done(evt_tx: &Sender<Event>, request_id: u64) {
    let _ = evt_tx.send(Event::RowsReadDone { request_id });
}

fn handle_export_rows(
    current: &SharedIndex,
    evt_tx: &Sender<Event>,
    path: String,
    rows: Vec<usize>,
    visible_columns: Vec<usize>,
) {
    let Some(index) = current_index(current) else {
        let _ = evt_tx.send(Event::Exported(Err("No CSV opened".to_string())));
        return;
    };
    let result = export_rows(
        &index.read().expect("current index read lock"),
        &path,
        &rows,
        &visible_columns,
    )
    .map(|()| path)
    .map_err(|e| e.to_string());
    let _ = evt_tx.send(Event::Exported(result));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CsvConfig, CsvIndex};
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_temp_csv(content: &[u8]) -> PathBuf {
        for attempt in 0..32u32 {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("csvfastview-worker-test-{id}-{attempt}.csv"));
            let file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path);
            if let Ok(mut file) = file {
                file.write_all(content).expect("write temp csv");
                return path;
            }
        }
        panic!("cannot allocate temp csv path")
    }

    #[test]
    fn filters_from_unique_index_with_include_and_exclude_modes() {
        let path = write_temp_csv(b"city,kind\nshanghai,a\nbeijing,b\nshenzhen,c\nshanghai,d\n");
        let index = CsvIndex::build(&path, CsvConfig::default()).expect("build index");
        let unique_index = index
            .index_unique_values_with_progress(0, |_, _| false)
            .expect("unique index");
        let mut unique_indexes = HashMap::new();
        unique_indexes.insert(0, Arc::new(unique_index));

        let mut include = HashMap::new();
        include.insert(
            0,
            UniqueFilter::Include(HashSet::from(["shanghai".to_string()])),
        );
        let include_rows = filter_from_unique_indexes(
            index.row_offsets.len(),
            &unique_indexes,
            &include,
            |_, _| false,
        )
        .expect("include filter");
        match include_rows {
            FilterRows::Rows(rows) => assert_eq!(rows, vec![0, 3]),
            FilterRows::AllExcept { .. } => {
                panic!("include filter should return explicit included rows")
            }
            FilterRows::All(_) => panic!("include filter should not return all rows"),
        }

        let mut exclude = HashMap::new();
        exclude.insert(
            0,
            UniqueFilter::Exclude(HashSet::from(["shanghai".to_string()])),
        );
        let exclude_rows = filter_from_unique_indexes(
            index.row_offsets.len(),
            &unique_indexes,
            &exclude,
            |_, _| false,
        )
        .expect("exclude filter");
        match exclude_rows {
            FilterRows::AllExcept { total, excluded } => {
                assert_eq!(total, 4);
                assert_eq!(excluded, vec![0, 3]);
            }
            FilterRows::Rows(_) => panic!("exclude filter should stay compact"),
            FilterRows::All(_) => panic!("exclude filter should not return all rows"),
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn exclude_filter_returns_rows_when_excluded_side_is_larger() {
        let path = write_temp_csv(b"city\nkeep\nskip1\nskip2\nskip3\n");
        let index = CsvIndex::build(&path, CsvConfig::default()).expect("build index");
        let unique_index = index
            .index_unique_values_with_progress(0, |_, _| false)
            .expect("unique index");
        let mut unique_indexes = HashMap::new();
        unique_indexes.insert(0, Arc::new(unique_index));

        let mut exclude = HashMap::new();
        exclude.insert(
            0,
            UniqueFilter::Exclude(HashSet::from([
                "skip1".to_string(),
                "skip2".to_string(),
                "skip3".to_string(),
            ])),
        );
        let rows = filter_from_unique_indexes(
            index.row_offsets.len(),
            &unique_indexes,
            &exclude,
            |_, _| false,
        )
        .expect("exclude filter");

        match rows {
            FilterRows::Rows(rows) => assert_eq!(rows, vec![0]),
            FilterRows::AllExcept { .. } => panic!("large exclusion should compact to rows"),
            FilterRows::All(_) => panic!("exclude filter should not return all rows"),
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn include_filter_returns_all_except_when_selected_side_is_larger() {
        let path = write_temp_csv(b"city\nkeep1\nkeep2\nkeep3\nskip\n");
        let index = CsvIndex::build(&path, CsvConfig::default()).expect("build index");
        let unique_index = index
            .index_unique_values_with_progress(0, |_, _| false)
            .expect("unique index");
        let mut unique_indexes = HashMap::new();
        unique_indexes.insert(0, Arc::new(unique_index));

        let mut include = HashMap::new();
        include.insert(
            0,
            UniqueFilter::Include(HashSet::from([
                "keep1".to_string(),
                "keep2".to_string(),
                "keep3".to_string(),
            ])),
        );
        let rows = filter_from_unique_indexes(
            index.row_offsets.len(),
            &unique_indexes,
            &include,
            |_, _| false,
        )
        .expect("include filter");

        match rows {
            FilterRows::AllExcept { total, excluded } => {
                assert_eq!(total, 4);
                assert_eq!(excluded, vec![3]);
            }
            FilterRows::Rows(_) => panic!("large include should compact to all-except"),
            FilterRows::All(_) => panic!("include filter should not return all rows"),
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn cached_read_file_reuses_handle_for_same_plan_path() {
        let path = write_temp_csv(b"name,age\nanna,18\nbob,20\n");
        let index = CsvIndex::build(&path, CsvConfig::default()).expect("build index");
        let plan = index.read_plan(&[0, 1]);
        let mut cache = None;

        let first = cached_read_file(&mut cache, &plan).expect("first file") as *mut File;
        let second = cached_read_file(&mut cache, &plan).expect("second file") as *mut File;

        assert_eq!(first, second);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn include_filter_builds_smaller_excluded_side_directly() {
        let path = write_temp_csv(b"city\nkeep1\nkeep2\nkeep3\nkeep4\nskip\n");
        let index = CsvIndex::build(&path, CsvConfig::default()).expect("build index");
        let unique_index = index
            .index_unique_values_with_progress(0, |_, _| false)
            .expect("unique index");
        let mut unique_indexes = HashMap::new();
        unique_indexes.insert(0, Arc::new(unique_index));

        let mut include = HashMap::new();
        include.insert(
            0,
            UniqueFilter::Include(HashSet::from([
                "keep1".to_string(),
                "keep2".to_string(),
                "keep3".to_string(),
                "keep4".to_string(),
            ])),
        );
        let rows = filter_from_unique_indexes(
            index.row_offsets.len(),
            &unique_indexes,
            &include,
            |_, _| false,
        )
        .expect("include filter");

        match rows {
            FilterRows::AllExcept { total, excluded } => {
                assert_eq!(total, 5);
                assert_eq!(excluded, vec![4]);
            }
            FilterRows::Rows(rows) => panic!("large include should not build rows: {rows:?}"),
            FilterRows::All(_) => panic!("include filter should not return all rows"),
        }

        let _ = fs::remove_file(path);
    }
}
