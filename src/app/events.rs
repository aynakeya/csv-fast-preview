use crate::worker::Event;

use super::format::format_bytes;
use super::state::CsvFastViewApp;

impl CsvFastViewApp {
    pub(super) fn apply_event(&mut self, evt: Event) {
        match evt {
            Event::Previewed(result) => match result {
                Ok(snapshot) => {
                    self.status = format!(
                        "Preview ready: first {} rows shown, indexing continues...",
                        snapshot.row_count
                    );
                    self.apply_headers(snapshot.headers);
                    self.total_rows = snapshot.row_count;
                    self.logical_rows = (0..snapshot.row_count).collect();
                    self.page_start = self
                        .page_start
                        .min(self.logical_rows.len().saturating_sub(1));
                    self.jump_to = self.page_start;
                    self.search_results.clear();
                    self.clear_rows();
                }
                Err(err) => self.status = format!("Preview failed: {err}"),
            },
            Event::IndexStarted(result) => match result {
                Ok((snapshot, total_bytes)) => {
                    let visible_rows = self.total_rows.max(snapshot.row_count);
                    self.status = format!("Indexing... {visible_rows} rows");
                    self.apply_headers(snapshot.headers);
                    self.total_rows = visible_rows;
                    if self.logical_rows.len() < visible_rows {
                        self.logical_rows
                            .extend(self.logical_rows.len()..visible_rows);
                    }
                    self.page_start = self
                        .page_start
                        .min(self.logical_rows.len().saturating_sub(1));
                    self.jump_to = self.page_start;
                    self.search_results.clear();
                    self.indexing = true;
                    self.index_progress = Some((0, 0, total_bytes));
                }
                Err(err) => {
                    self.indexing = false;
                    self.index_progress = None;
                    self.status = format!("Index start failed: {err}");
                }
            },
            Event::IndexProgress {
                indexed_rows,
                bytes,
                total_bytes,
            } => {
                let old_len = self.total_rows;
                self.total_rows = indexed_rows;
                if self.logical_rows.len() == old_len {
                    self.logical_rows.extend(old_len..indexed_rows);
                } else if self.logical_rows.len() < indexed_rows {
                    self.logical_rows = (0..indexed_rows).collect();
                    self.clear_rows();
                }
                self.indexing = true;
                self.index_progress = Some((indexed_rows, bytes, total_bytes));
                self.status = if total_bytes > 0 {
                    format!(
                        "Indexing... {} rows, {}/{} ({:.1}%)",
                        indexed_rows,
                        format_bytes(bytes),
                        format_bytes(total_bytes),
                        bytes as f64 * 100.0 / total_bytes as f64
                    )
                } else {
                    format!("Indexing... {indexed_rows} rows")
                };
            }
            Event::Opened(result) => match result {
                Ok(snapshot) => {
                    self.status = format!(
                        "Opened: {} rows, {} columns",
                        snapshot.row_count,
                        snapshot.headers.len()
                    );
                    self.apply_headers(snapshot.headers);
                    self.total_rows = snapshot.row_count;
                    self.logical_rows = (0..snapshot.row_count).collect();
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.search_results.clear();
                    self.indexing = false;
                    self.index_progress = None;
                }
                Err(err) => {
                    self.indexing = false;
                    self.index_progress = None;
                    self.status = format!("Open failed: {err}");
                }
            },
            Event::Filtered(result) => match result {
                Ok(rows) => {
                    self.search_results = rows.clone();
                    self.logical_rows = rows;
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                    self.status = format!("Filter done: {} rows", self.logical_rows.len());
                    self.filtering = false;
                    self.filter_progress = None;
                }
                Err(err) => {
                    self.status = format!("Filter failed: {err}");
                    self.filtering = false;
                    self.filter_progress = None;
                }
            },
            Event::FilterProgress { done, total } => {
                self.filter_progress = Some((done, total));
                self.filtering = true;
                if total > 0 {
                    self.status = format!(
                        "Filtering... {done}/{total} ({:.1}%)",
                        done as f64 * 100.0 / total as f64
                    );
                } else {
                    self.status = format!("Filtering... {done}");
                }
            }
            Event::FilterCancelled => {
                self.filtering = false;
                self.filter_progress = None;
                self.status = "Filter cancelled".to_string();
            }
            Event::Searched(result) => match result {
                Ok(rows) => {
                    self.search_results = rows;
                    self.searching = false;
                    self.search_progress = None;
                    self.status = format!("Search done: {} hits", self.search_results.len());
                }
                Err(err) => {
                    self.searching = false;
                    self.search_progress = None;
                    self.status = format!("Search failed: {err}");
                }
            },
            Event::SearchProgress { done, total } => {
                self.searching = true;
                self.search_progress = Some((done, total));
                self.status = if total > 0 {
                    format!(
                        "Searching... {done}/{total} ({:.1}%)",
                        done as f64 * 100.0 / total as f64
                    )
                } else {
                    format!("Searching... {done}")
                };
            }
            Event::SearchCancelled => {
                self.searching = false;
                self.search_progress = None;
                self.status = "Search cancelled".to_string();
            }
            Event::RowsRead { request_id, rows } => {
                if request_id >= self.row_request_floor {
                    for (logical_idx, row) in rows {
                        self.insert_loaded_row(logical_idx, row);
                    }
                }
            }
            Event::RowsReadDone { request_id } => {
                if request_id == self.row_request_id {
                    self.requested_range = None;
                }
            }
            Event::Exported(result) => match result {
                Ok(path) => self.status = format!("Exported: {path}"),
                Err(err) => self.status = format!("Export failed: {err}"),
            },
        }
    }

    fn apply_headers(&mut self, headers: Vec<String>) {
        if self.visible_columns.len() != headers.len() {
            self.visible_columns = vec![true; headers.len()];
            self.column_widths = vec![120.0; headers.len()];
        }
        self.headers = headers;
    }
}
