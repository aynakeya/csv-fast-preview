use crate::core::{CsvConfig, FilterMode};

use super::snapshot::CsvSnapshot;

pub enum Job {
    OpenFile {
        path: String,
        config: CsvConfig,
    },
    Filter {
        col: usize,
        keyword: String,
        mode: FilterMode,
    },
    Search {
        keyword: String,
    },
    ReadRows {
        request_id: u64,
        start: usize,
        rows: Vec<usize>,
    },
    ExportRows {
        path: String,
        rows: Vec<usize>,
        visible_columns: Vec<usize>,
    },
}

pub enum Event {
    Previewed(Result<CsvSnapshot, String>),
    IndexStarted(Result<(CsvSnapshot, u64), String>),
    IndexProgress {
        indexed_rows: usize,
        bytes: u64,
        total_bytes: u64,
    },
    Opened(Result<CsvSnapshot, String>),
    RowsRead {
        request_id: u64,
        start: usize,
        rows: Vec<(usize, Vec<String>)>,
    },
    Exported(Result<String, String>),
    Filtered(Result<Vec<usize>, String>),
    FilterProgress {
        done: usize,
        total: usize,
    },
    FilterCancelled,
    Searched(Result<Vec<usize>, String>),
    SearchProgress {
        done: usize,
        total: usize,
    },
    SearchCancelled,
}
