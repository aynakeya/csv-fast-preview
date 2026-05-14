use crate::core::{CsvConfig, UniqueValue};
use std::collections::{HashMap, HashSet};

use super::snapshot::CsvSnapshot;

pub enum Job {
    OpenFile {
        path: String,
        config: CsvConfig,
    },
    IndexUnique {
        col: usize,
    },
    ApplyUniqueFilters {
        filters: HashMap<usize, HashSet<String>>,
    },
    ReadRows {
        request_id: u64,
        rows: Vec<(usize, usize)>,
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
        rows: Vec<(usize, Vec<String>)>,
    },
    RowsReadDone {
        request_id: u64,
    },
    Exported(Result<String, String>),
    Filtered(Result<Vec<usize>, String>),
    FilterProgress {
        done: usize,
        total: usize,
    },
    FilterCancelled,
    UniqueIndexed {
        col: usize,
        result: Result<Vec<UniqueValue>, String>,
    },
    UniqueIndexProgress {
        col: usize,
        done: usize,
        total: usize,
    },
}
