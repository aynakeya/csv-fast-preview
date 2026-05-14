use crate::core::{CsvConfig, CsvEncoding, UniqueValue};
use crate::worker::{Job, Worker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use super::constants::{ROW_CACHE_AFTER, ROW_CACHE_BEFORE, ROW_CACHE_LIMIT};

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
    pub(super) logical_rows: Vec<usize>,
    pub(super) page_start: usize,
    pub(super) jump_to: usize,
    pub(super) row_cache: HashMap<usize, Vec<String>>,
    pub(super) row_cache_order: VecDeque<usize>,
    pub(super) row_request_id: u64,
    pub(super) row_request_floor: u64,
    pub(super) requested_range: Option<(usize, usize)>,
    pub(super) scroll_to_row: Option<usize>,
    pub(super) pending_dropped_path: Option<PathBuf>,

    pub(super) visible_columns: Vec<bool>,
    pub(super) column_widths: Vec<f32>,

    pub(super) filtering: bool,
    pub(super) filter_progress: Option<(usize, usize)>,
    pub(super) unique_columns: HashMap<usize, UniqueColumnState>,
    pub(super) active_filter_column: Option<usize>,
    pub(super) indexing: bool,
    pub(super) index_progress: Option<(usize, u64, u64)>,
    pub(super) selected_cell: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UniqueColumnState {
    pub values: Vec<UniqueValue>,
    pub selected: HashSet<String>,
    pub indexing: bool,
    pub progress: Option<(usize, usize)>,
    pub error: Option<String>,
    pub value_filter: String,
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
            logical_rows: Vec::new(),
            page_start: 0,
            jump_to: 0,
            row_cache: HashMap::new(),
            row_cache_order: VecDeque::new(),
            row_request_id: 0,
            row_request_floor: 0,
            requested_range: None,
            scroll_to_row: None,
            pending_dropped_path: None,
            visible_columns: Vec::new(),
            column_widths: Vec::new(),
            filtering: false,
            filter_progress: None,
            unique_columns: HashMap::new(),
            active_filter_column: None,
            indexing: false,
            index_progress: None,
            selected_cell: None,
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
        self.row_request_id = self.row_request_id.wrapping_add(1);
        self.row_request_floor = self.row_request_id;
        self.requested_range = None;
    }

    pub(super) fn request_cached_row(&mut self, logical_idx: usize) {
        if self.row_cache.contains_key(&logical_idx) || self.logical_rows.get(logical_idx).is_none()
        {
            return;
        }

        if !self
            .requested_range
            .is_some_and(|(start, end)| (start..end).contains(&logical_idx))
        {
            let (start, end) = self.cache_window_for(logical_idx);
            let rows = self.cache_window_rows(logical_idx, start, end);
            self.row_request_id = self.row_request_id.wrapping_add(1);
            self.requested_range = Some((start, end));
            let _ = self.worker.tx.send(Job::ReadRows {
                request_id: self.row_request_id,
                rows,
            });
        }
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
            rows.push((idx, self.logical_rows[idx]));
        }
        for idx in (start..logical_idx).rev() {
            rows.push((idx, self.logical_rows[idx]));
        }
        rows
    }

    pub(super) fn insert_loaded_row(&mut self, logical_idx: usize, row: Vec<String>) {
        if !self.row_cache.contains_key(&logical_idx) {
            self.row_cache_order.push_back(logical_idx);
        }
        self.row_cache.insert(logical_idx, row);
        while self.row_cache_order.len() > ROW_CACHE_LIMIT {
            if let Some(old) = self.row_cache_order.pop_front() {
                self.row_cache.remove(&old);
            }
        }
    }

    pub(super) fn visible_column_indices(&self) -> Vec<usize> {
        self.visible_columns
            .iter()
            .enumerate()
            .filter_map(|(i, v)| if *v { Some(i) } else { None })
            .collect()
    }

    pub(super) fn selected_unique_filters(&self) -> HashMap<usize, HashSet<String>> {
        self.unique_columns
            .iter()
            .filter_map(|(col, state)| {
                if state.selected.is_empty() || state.selected.len() == state.values.len() {
                    None
                } else {
                    Some((*col, state.selected.clone()))
                }
            })
            .collect()
    }

    pub(super) fn has_selected_unique_filters(&self) -> bool {
        self.unique_columns
            .values()
            .any(|state| !state.selected.is_empty())
    }
}
