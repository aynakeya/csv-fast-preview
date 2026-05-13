use std::collections::{HashMap, VecDeque};

const WORKER_ROW_CACHE_LIMIT: usize = 8_000;

pub(super) struct RowCache {
    rows: HashMap<usize, Vec<String>>,
    order: VecDeque<usize>,
}

impl RowCache {
    pub(super) fn new() -> Self {
        Self {
            rows: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.order.clear();
    }

    pub(super) fn get(&self, real_row: usize) -> Option<&Vec<String>> {
        self.rows.get(&real_row)
    }

    pub(super) fn insert(&mut self, real_row: usize, row: Vec<String>) {
        if !self.rows.contains_key(&real_row) {
            self.order.push_back(real_row);
        }
        self.rows.insert(real_row, row);
        while self.order.len() > WORKER_ROW_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.rows.remove(&old);
            }
        }
    }
}
