use crate::core::{CsvConfig, UniqueValue};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::snapshot::CsvSnapshot;

pub enum FilterRows {
    All(usize),
    AllExcept { total: usize, excluded: Vec<usize> },
    Rows(Vec<usize>),
}

pub enum UniqueFilter {
    Include(HashSet<String>),
    Exclude(HashSet<String>),
}

pub enum Job {
    OpenFile {
        path: String,
        config: CsvConfig,
    },
    IndexUnique {
        col: usize,
    },
    ApplyUniqueFilters {
        filters: HashMap<usize, UniqueFilter>,
    },
    ReadRows {
        request_id: u64,
        rows: Vec<(usize, usize)>,
        columns: Vec<usize>,
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
        rows: Vec<(usize, Vec<(usize, String)>)>,
    },
    RowsReadDone {
        request_id: u64,
    },
    Exported(Result<String, String>),
    Filtered(Result<FilterRows, String>),
    FilterProgress {
        done: usize,
        total: usize,
    },
    FilterCancelled,
    UniqueIndexed {
        col: usize,
        result: Result<Arc<[UniqueValue]>, String>,
    },
    UniqueIndexProgress {
        col: usize,
        done: usize,
        total: usize,
    },
}
