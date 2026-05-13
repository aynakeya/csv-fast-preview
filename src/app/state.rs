use crate::core::{CsvConfig, CsvEncoding, FilterMode};
use crate::worker::{Job, Worker};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use super::constants::{ROW_CACHE_LIMIT, ROW_PREFETCH_COUNT};

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
    pub(super) requested_range: Option<(usize, usize)>,
    pub(super) scroll_to_row: Option<usize>,

    pub(super) visible_columns: Vec<bool>,
    pub(super) column_widths: Vec<f32>,

    pub(super) filter_column: usize,
    pub(super) filter_keyword: String,
    pub(super) filter_mode: FilterMode,
    pub(super) filtering: bool,
    pub(super) filter_progress: Option<(usize, usize)>,
    pub(super) indexing: bool,
    pub(super) index_progress: Option<(usize, u64, u64)>,
    pub(super) search_keyword: String,
    pub(super) searching: bool,
    pub(super) search_progress: Option<(usize, usize)>,
    pub(super) search_results: Vec<usize>,
    pub(super) selected_cell: Option<String>,
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
            requested_range: None,
            scroll_to_row: None,
            visible_columns: Vec::new(),
            column_widths: Vec::new(),
            filter_column: 0,
            filter_keyword: String::new(),
            filter_mode: FilterMode::Contains,
            filtering: false,
            filter_progress: None,
            indexing: false,
            index_progress: None,
            search_keyword: String::new(),
            searching: false,
            search_progress: None,
            search_results: Vec::new(),
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
        let _ = self.worker.tx.send(Job::OpenFile {
            path: self.path.clone(),
            config: cfg,
        });
    }

    pub(super) fn clear_rows(&mut self) {
        self.row_cache.clear();
        self.row_cache_order.clear();
        self.row_request_id = self.row_request_id.wrapping_add(1);
        self.requested_range = None;
    }

    pub(super) fn read_cached_row(&mut self, logical_idx: usize) -> Vec<String> {
        if let Some(row) = self.row_cache.get(&logical_idx) {
            return row.clone();
        }

        if self.logical_rows.get(logical_idx).is_none() {
            return Vec::new();
        }

        if !self
            .requested_range
            .is_some_and(|(start, end)| (start..end).contains(&logical_idx))
        {
            let prefetch_len =
                ROW_PREFETCH_COUNT.min(self.logical_rows.len().saturating_sub(logical_idx));
            let rows = self.logical_rows[logical_idx..logical_idx + prefetch_len].to_vec();
            self.row_request_id = self.row_request_id.wrapping_add(1);
            self.requested_range = Some((logical_idx, logical_idx + prefetch_len));
            let _ = self.worker.tx.send(Job::ReadRows {
                request_id: self.row_request_id,
                start: logical_idx,
                rows,
            });
        }

        Vec::new()
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
}
