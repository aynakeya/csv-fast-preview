use crate::core::{CsvConfig, CsvEncoding, UniqueValue};
use crate::worker::{Job, UniqueFilter, Worker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use super::constants::TABLE_ROW_HEADER_WIDTH;
use super::constants::{
    ROW_CACHE_AFTER, ROW_CACHE_BEFORE, ROW_CACHE_LIMIT, ROW_CACHE_ORDER_COMPACT_FACTOR,
    ROW_CACHE_TOUCH_STRIDE,
};

pub(crate) struct CsvFastViewApp {
    pub(super) worker: Worker,
    pub(super) path: String,
    pub(super) delimiter: String,
    pub(super) quote: String,
    pub(super) skip_lines: usize,
    pub(super) has_headers: bool,
    pub(super) flexible: bool,
    pub(super) encoding: CsvEncoding,
    pub(super) status: String,
    pub(super) file_size_text: String,
    pub(super) export_path: String,

    pub(super) headers: Vec<String>,
    pub(super) total_rows: usize,
    pub(super) logical_rows: LogicalRows,
    pub(super) page_start: usize,
    pub(super) jump_to: usize,
    pub(super) row_cache: HashMap<usize, CachedRow>,
    pub(super) row_cache_order: VecDeque<(usize, u64)>,
    pub(super) row_cache_access: u64,
    pub(super) row_cache_columns: Vec<usize>,
    pub(super) row_request_id: u64,
    pub(super) row_request_floor: u64,
    pub(super) requested_range: Option<(usize, usize, Vec<usize>)>,
    pub(super) scroll_to_row: Option<usize>,
    pub(super) pending_dropped_path: Option<PathBuf>,

    pub(super) visible_columns: Vec<bool>,
    pub(super) column_widths: Vec<f32>,
    pub(super) column_layout: Vec<ColumnLayoutEntry>,
    pub(super) column_layout_dirty: bool,
    pub(super) table_total_width: f32,

    pub(super) filtering: bool,
    pub(super) filter_progress: Option<(usize, usize)>,
    pub(super) unique_columns: HashMap<usize, UniqueColumnState>,
    pub(super) active_filter_column: Option<usize>,
    pub(super) indexing: bool,
    pub(super) index_progress: Option<(usize, u64, u64)>,
    pub(super) selected_cell: Option<String>,
    pub(super) rendered_table_columns: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ColumnLayoutEntry {
    pub col_idx: usize,
    pub width: f32,
    pub min_x: f32,
    pub max_x: f32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CachedRow {
    cells: Vec<(usize, String)>,
    access_id: u64,
}

impl CachedRow {
    pub(super) fn has_columns(&self, columns: &[usize]) -> bool {
        columns.iter().all(|col| {
            self.cells
                .binary_search_by_key(col, |(idx, _)| *idx)
                .is_ok()
        })
    }

    pub(super) fn get(&self, col: usize) -> Option<&str> {
        self.cells
            .binary_search_by_key(&col, |(idx, _)| *idx)
            .ok()
            .map(|idx| self.cells[idx].1.as_str())
    }

    fn retain_columns(&mut self, columns: &[usize]) {
        self.cells
            .retain(|(col, _)| columns.binary_search(col).is_ok());
    }

    fn insert_cells(&mut self, cells: Vec<(usize, String)>) {
        if self.cells.is_empty() {
            self.cells = cells;
            return;
        }

        let mut incoming = cells.into_iter().peekable();
        let mut old = std::mem::take(&mut self.cells).into_iter().peekable();
        while old.peek().is_some() || incoming.peek().is_some() {
            match (old.peek(), incoming.peek()) {
                (Some((old_col, _)), Some((new_col, _))) => match old_col.cmp(new_col) {
                    std::cmp::Ordering::Less => {
                        self.cells.push(old.next().expect("old row cell"));
                    }
                    std::cmp::Ordering::Greater => {
                        self.cells.push(incoming.next().expect("new row cell"));
                    }
                    std::cmp::Ordering::Equal => {
                        let _ = old.next();
                        self.cells.push(incoming.next().expect("new row cell"));
                    }
                },
                (Some(_), None) => {
                    self.cells.extend(old);
                    break;
                }
                (None, Some(_)) => {
                    self.cells.extend(incoming);
                    break;
                }
                (None, None) => break,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum LogicalRows {
    All(usize),
    AllExcept { total: usize, excluded: Vec<usize> },
    Filtered(Vec<usize>),
}

impl Default for LogicalRows {
    fn default() -> Self {
        Self::All(0)
    }
}

impl LogicalRows {
    pub fn len(&self) -> usize {
        match self {
            Self::All(len) => *len,
            Self::AllExcept { total, excluded } => total.saturating_sub(excluded.len()),
            Self::Filtered(rows) => rows.len(),
        }
    }

    pub fn get(&self, logical_idx: usize) -> Option<usize> {
        match self {
            Self::All(len) => (logical_idx < *len).then_some(logical_idx),
            Self::AllExcept { total, excluded } => {
                kth_row_not_in_excluded(*total, excluded, logical_idx)
            }
            Self::Filtered(rows) => rows.get(logical_idx).copied(),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Self::All(_))
    }

    pub fn slice_to_vec(&self, start: usize, end: usize) -> Vec<usize> {
        match self {
            Self::All(_) => (start..end).collect(),
            Self::AllExcept { .. } => (start..end).filter_map(|idx| self.get(idx)).collect(),
            Self::Filtered(rows) => rows[start..end].to_vec(),
        }
    }
}

fn kth_row_not_in_excluded(total: usize, excluded: &[usize], logical_idx: usize) -> Option<usize> {
    if logical_idx >= total.saturating_sub(excluded.len()) {
        return None;
    }

    let mut lo = 0usize;
    let mut hi = total;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let excluded_through_mid = excluded.partition_point(|row| *row <= mid);
        let visible_through_mid = mid + 1 - excluded_through_mid;
        if visible_through_mid > logical_idx {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UniqueColumnState {
    pub values: Arc<[UniqueValue]>,
    pub selected: HashSet<String>,
    pub excluded: HashSet<String>,
    pub all_selected: bool,
    pub indexing: bool,
    pub progress: Option<(usize, usize)>,
    pub error: Option<String>,
    pub value_filter: String,
    pub cached_value_filter: String,
    pub cached_filter_value_count: usize,
    pub cached_filtered_indices: Vec<usize>,
}

impl Default for CsvFastViewApp {
    fn default() -> Self {
        Self {
            worker: Worker::spawn(),
            path: String::new(),
            delimiter: ",".into(),
            quote: "\"".into(),
            skip_lines: 0,
            has_headers: true,
            flexible: true,
            encoding: CsvEncoding::Utf8,
            status: "Ready".to_string(),
            file_size_text: "-".to_string(),
            export_path: "/tmp/csvfastview-export.csv".to_string(),
            headers: Vec::new(),
            total_rows: 0,
            logical_rows: LogicalRows::default(),
            page_start: 0,
            jump_to: 0,
            row_cache: HashMap::new(),
            row_cache_order: VecDeque::new(),
            row_cache_access: 0,
            row_cache_columns: Vec::new(),
            row_request_id: 0,
            row_request_floor: 0,
            requested_range: None,
            scroll_to_row: None,
            pending_dropped_path: None,
            visible_columns: Vec::new(),
            column_widths: Vec::new(),
            column_layout: Vec::new(),
            column_layout_dirty: true,
            table_total_width: TABLE_ROW_HEADER_WIDTH,
            filtering: false,
            filter_progress: None,
            unique_columns: HashMap::new(),
            active_filter_column: None,
            indexing: false,
            index_progress: None,
            selected_cell: None,
            rendered_table_columns: 0,
        }
    }
}

impl CsvFastViewApp {
    pub(super) fn parse_config(&self) -> CsvConfig {
        let delimiter = self.delimiter.bytes().next().unwrap_or(b',');
        let quote = self.quote.bytes().next().unwrap_or(b'"');
        CsvConfig {
            delimiter,
            has_headers: self.has_headers,
            quote,
            skip_lines: self.skip_lines,
            flexible: self.flexible,
            encoding: self.encoding,
        }
    }

    pub(super) fn open_path(&mut self, path: PathBuf) {
        self.path = path.display().to_string();
        self.open_current_file();
    }

    pub(super) fn open_current_file(&mut self) {
        let cfg = self.parse_config();
        self.file_size_text = std::fs::metadata(&self.path)
            .map(|meta| format!("{} bytes", meta.len()))
            .unwrap_or_else(|_| "-".to_string());
        self.status = "Indexing in background...".to_string();
        self.indexing = true;
        self.index_progress = None;
        self.clear_rows();
        self.unique_columns.clear();
        self.active_filter_column = None;
        let _ = self.worker.tx.send(Job::OpenFile {
            path: self.path.clone(),
            config: cfg,
        });
    }

    pub(super) fn clear_rows(&mut self) {
        self.row_cache.clear();
        self.row_cache_order.clear();
        self.row_cache_access = 0;
        self.row_cache_columns.clear();
        self.row_request_id = self.row_request_id.wrapping_add(1);
        self.row_request_floor = self.row_request_id;
        self.requested_range = None;
    }

    pub(super) fn request_cached_row(&mut self, logical_idx: usize, columns: &[usize]) {
        if columns.is_empty() || self.logical_rows.get(logical_idx).is_none() {
            return;
        }
        if self.row_has_columns(logical_idx, columns) {
            self.touch_cached_row(logical_idx);
            return;
        }

        if !self
            .requested_range
            .as_ref()
            .is_some_and(|(start, end, requested_columns)| {
                (*start..*end).contains(&logical_idx)
                    && columns
                        .iter()
                        .all(|col| requested_columns.binary_search(col).is_ok())
            })
        {
            let (start, end) = self.cache_window_for(logical_idx);
            let rows =
                self.cache_window_rows_with_missing_columns(logical_idx, start, end, columns);
            if rows.is_empty() {
                return;
            }
            let mut requested_columns = columns.to_vec();
            requested_columns.sort_unstable();
            requested_columns.dedup();
            self.retain_cached_columns_if_changed(&requested_columns);
            self.row_request_id = self.row_request_id.wrapping_add(1);
            self.requested_range = Some((start, end, requested_columns.clone()));
            let _ = self.worker.tx.send(Job::ReadRows {
                request_id: self.row_request_id,
                rows,
                columns: requested_columns,
            });
        }
    }

    fn row_has_columns(&self, logical_idx: usize, columns: &[usize]) -> bool {
        let Some(row) = self.row_cache.get(&logical_idx) else {
            return false;
        };
        row.has_columns(columns)
    }

    fn retain_cached_columns_if_changed(&mut self, columns: &[usize]) {
        if self.row_cache_columns == columns {
            return;
        }
        for row in self.row_cache.values_mut() {
            row.retain_columns(columns);
        }
        self.row_cache_columns = columns.to_vec();
    }

    fn cache_window_for(&self, logical_idx: usize) -> (usize, usize) {
        let start = logical_idx.saturating_sub(ROW_CACHE_BEFORE);
        let end = (logical_idx + ROW_CACHE_AFTER + 1).min(self.logical_rows.len());
        (start, end)
    }

    fn cache_window_rows(
        &self,
        logical_idx: usize,
        start: usize,
        end: usize,
    ) -> Vec<(usize, usize)> {
        let mut rows = Vec::with_capacity(end.saturating_sub(start));
        for idx in logical_idx..end {
            if let Some(real_row) = self.logical_rows.get(idx) {
                rows.push((idx, real_row));
            }
        }
        for idx in (start..logical_idx).rev() {
            if let Some(real_row) = self.logical_rows.get(idx) {
                rows.push((idx, real_row));
            }
        }
        rows
    }

    fn cache_window_rows_with_missing_columns(
        &self,
        logical_idx: usize,
        start: usize,
        end: usize,
        columns: &[usize],
    ) -> Vec<(usize, usize)> {
        self.cache_window_rows(logical_idx, start, end)
            .into_iter()
            .filter(|(idx, _)| !self.row_has_columns(*idx, columns))
            .collect()
    }

    pub(super) fn insert_loaded_cells(&mut self, logical_idx: usize, cells: Vec<(usize, String)>) {
        if cells.is_empty() {
            return;
        }
        let access_id = self.next_row_cache_access();
        let row = self.row_cache.entry(logical_idx).or_default();
        row.access_id = access_id;
        row.insert_cells(cells);
        self.row_cache_order.push_back((logical_idx, access_id));
        self.evict_old_cached_rows();
    }

    fn touch_cached_row(&mut self, logical_idx: usize) {
        let should_touch = self.row_cache.get(&logical_idx).is_some_and(|row| {
            self.row_cache_access.saturating_sub(row.access_id) >= ROW_CACHE_TOUCH_STRIDE
        });
        if should_touch {
            let access_id = self.next_row_cache_access();
            if let Some(row) = self.row_cache.get_mut(&logical_idx) {
                row.access_id = access_id;
                self.row_cache_order.push_back((logical_idx, access_id));
                self.compact_row_cache_order_if_needed();
            }
        }
    }

    fn next_row_cache_access(&mut self) -> u64 {
        self.row_cache_access = self.row_cache_access.wrapping_add(1).max(1);
        self.row_cache_access
    }

    fn evict_old_cached_rows(&mut self) {
        while self.row_cache.len() > ROW_CACHE_LIMIT {
            let Some((old_idx, old_access)) = self.row_cache_order.pop_front() else {
                break;
            };
            if self
                .row_cache
                .get(&old_idx)
                .is_some_and(|row| row.access_id == old_access)
            {
                self.row_cache.remove(&old_idx);
            }
        }
        self.compact_row_cache_order_if_needed();
    }

    fn compact_row_cache_order_if_needed(&mut self) {
        let max_order_len = ROW_CACHE_LIMIT.saturating_mul(ROW_CACHE_ORDER_COMPACT_FACTOR);
        if self.row_cache_order.len() <= max_order_len {
            return;
        }
        let mut compacted: Vec<_> = self
            .row_cache
            .iter()
            .map(|(idx, row)| (*idx, row.access_id))
            .collect();
        compacted.sort_by_key(|(_, access_id)| *access_id);
        self.row_cache_order = compacted.into();
    }

    pub(super) fn visible_column_indices(&self) -> Vec<usize> {
        self.visible_columns
            .iter()
            .enumerate()
            .filter_map(|(i, v)| if *v { Some(i) } else { None })
            .collect()
    }

    pub(super) fn mark_column_layout_dirty(&mut self) {
        self.column_layout_dirty = true;
    }

    pub(super) fn rebuild_column_layout_if_needed(&mut self) {
        if !self.column_layout_dirty {
            return;
        }

        self.column_layout.clear();
        let mut x = TABLE_ROW_HEADER_WIDTH;
        for (col_idx, is_visible) in self.visible_columns.iter().copied().enumerate() {
            if !is_visible {
                continue;
            }
            let width = self.column_widths[col_idx];
            let min_x = x;
            let max_x = min_x + width;
            self.column_layout.push(ColumnLayoutEntry {
                col_idx,
                width,
                min_x,
                max_x,
            });
            x = max_x;
        }
        self.table_total_width = x;
        self.column_layout_dirty = false;
    }

    pub(super) fn set_all_rows(&mut self, len: usize) {
        self.logical_rows = LogicalRows::All(len);
    }

    pub(super) fn set_filtered_rows(&mut self, rows: Vec<usize>) {
        self.logical_rows = LogicalRows::Filtered(rows);
    }

    pub(super) fn set_all_except_rows(&mut self, total: usize, mut excluded: Vec<usize>) {
        excluded.sort_unstable();
        excluded.dedup();
        self.logical_rows = LogicalRows::AllExcept { total, excluded };
    }

    pub(super) fn selected_unique_filters(&self) -> HashMap<usize, UniqueFilter> {
        self.unique_columns
            .iter()
            .filter_map(|(col, state)| {
                if state.all_selected {
                    (!state.excluded.is_empty())
                        .then(|| (*col, UniqueFilter::Exclude(state.excluded.clone())))
                } else if state.selected.is_empty() {
                    None
                } else {
                    Some((*col, UniqueFilter::Include(state.selected.clone())))
                }
            })
            .collect()
    }

    pub(super) fn has_selected_unique_filters(&self) -> bool {
        self.unique_columns.values().any(|state| {
            if state.all_selected {
                !state.excluded.is_empty()
            } else {
                !state.selected.is_empty()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CsvFastViewApp, LogicalRows};
    use crate::app::constants::{ROW_CACHE_LIMIT, ROW_CACHE_TOUCH_STRIDE};

    #[test]
    fn all_except_rows_map_logical_indices_to_visible_source_rows() {
        let rows = LogicalRows::AllExcept {
            total: 8,
            excluded: vec![0, 3, 6],
        };

        assert_eq!(rows.len(), 5);
        assert_eq!(rows.get(0), Some(1));
        assert_eq!(rows.get(1), Some(2));
        assert_eq!(rows.get(2), Some(4));
        assert_eq!(rows.get(3), Some(5));
        assert_eq!(rows.get(4), Some(7));
        assert_eq!(rows.get(5), None);
        assert_eq!(rows.slice_to_vec(1, 4), vec![2, 4, 5]);
    }

    #[test]
    fn row_cache_eviction_keeps_recently_touched_rows() {
        let mut app = CsvFastViewApp::default();
        app.set_all_rows(ROW_CACHE_LIMIT + 1);
        for row in 0..ROW_CACHE_LIMIT {
            app.insert_loaded_cells(row, vec![(0, row.to_string())]);
        }

        app.request_cached_row(0, &[0]);
        app.insert_loaded_cells(ROW_CACHE_LIMIT, vec![(0, "new".to_string())]);

        assert!(app.row_cache.contains_key(&0));
        assert!(!app.row_cache.contains_key(&1));
        assert_eq!(app.row_cache.len(), ROW_CACHE_LIMIT);
    }

    #[test]
    fn row_cache_order_compaction_preserves_lru_order() {
        let mut app = CsvFastViewApp::default();
        app.set_all_rows(3);
        app.insert_loaded_cells(0, vec![(0, "0".to_string())]);
        app.insert_loaded_cells(1, vec![(0, "1".to_string())]);
        app.insert_loaded_cells(2, vec![(0, "2".to_string())]);
        app.row_cache_access += ROW_CACHE_TOUCH_STRIDE;
        app.request_cached_row(1, &[0]);
        app.row_cache_access += ROW_CACHE_TOUCH_STRIDE;
        app.request_cached_row(0, &[0]);
        for _ in 0..=ROW_CACHE_LIMIT * crate::app::constants::ROW_CACHE_ORDER_COMPACT_FACTOR {
            app.row_cache_order.push_back((99, 1));
        }

        app.compact_row_cache_order_if_needed();

        let ordered_rows = app
            .row_cache_order
            .iter()
            .map(|(row, _)| *row)
            .collect::<Vec<_>>();
        assert_eq!(ordered_rows, vec![2, 1, 0]);
    }

    #[test]
    fn row_cache_touch_skips_recent_rows() {
        let mut app = CsvFastViewApp::default();
        app.set_all_rows(1);
        app.insert_loaded_cells(0, vec![(0, "0".to_string())]);
        let order_len = app.row_cache_order.len();
        app.request_cached_row(0, &[0]);

        assert_eq!(app.row_cache_order.len(), order_len);
    }
}
