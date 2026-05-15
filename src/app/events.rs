use crate::worker::{Event, FilterRows};

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
                    self.set_all_rows(snapshot.row_count);
                    self.page_start = self
                        .page_start
                        .min(self.logical_rows.len().saturating_sub(1));
                    self.jump_to = self.page_start;
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
                    if self.logical_rows.is_all() {
                        self.set_all_rows(visible_rows);
                    }
                    self.page_start = self
                        .page_start
                        .min(self.logical_rows.len().saturating_sub(1));
                    self.jump_to = self.page_start;
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
                self.total_rows = indexed_rows;
                if self.logical_rows.is_all() {
                    self.set_all_rows(indexed_rows);
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
                    self.set_all_rows(snapshot.row_count);
                    self.page_start = 0;
                    self.jump_to = 0;
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
                Ok(FilterRows::All(len)) => {
                    self.set_all_rows(len);
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                    self.status = format!("Filter done: {} rows", self.logical_rows.len());
                    self.filtering = false;
                    self.filter_progress = None;
                }
                Ok(FilterRows::AllExcept { total, excluded }) => {
                    self.set_all_except_rows(total, excluded);
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                    self.status = format!("Filter done: {} rows", self.logical_rows.len());
                    self.filtering = false;
                    self.filter_progress = None;
                }
                Ok(FilterRows::Rows(rows)) => {
                    let row_count = rows.len();
                    self.set_filtered_rows(rows);
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                    self.status = format!("Filter done: {row_count} rows");
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
            Event::UniqueIndexProgress { col, done, total } => {
                let state = self.unique_columns.entry(col).or_default();
                state.indexing = true;
                state.progress = Some((done, total));
                state.error = None;
                self.status = if total > 0 {
                    format!(
                        "Indexing unique values for column {col}: {done}/{total} ({:.1}%)",
                        done as f64 * 100.0 / total as f64
                    )
                } else {
                    format!("Indexing unique values for column {col}: {done}")
                };
            }
            Event::UniqueIndexed { col, result } => {
                let state = self.unique_columns.entry(col).or_default();
                state.indexing = false;
                state.progress = None;
                match result {
                    Ok(values) => {
                        let known: std::collections::HashSet<&str> =
                            values.iter().map(|item| item.value.as_str()).collect();
                        state
                            .selected
                            .retain(|value| known.contains(value.as_str()));
                        state
                            .excluded
                            .retain(|value| known.contains(value.as_str()));
                        if values.is_empty() {
                            state.all_selected = false;
                        }
                        state.values = values;
                        state.cached_value_filter.clear();
                        state.cached_filter_value_count = 0;
                        state.cached_filtered_indices.clear();
                        state.error = None;
                        self.active_filter_column = Some(col);
                        self.status = format!(
                            "Unique index ready for column {col}: {} values",
                            state.values.len()
                        );
                    }
                    Err(err) => {
                        state.error = Some(err.clone());
                        self.status = format!("Unique index failed for column {col}: {err}");
                    }
                }
            }
            Event::RowsRead { request_id, rows } => {
                if request_id == self.row_request_id && request_id >= self.row_request_floor {
                    for (logical_idx, cells) in rows {
                        self.insert_loaded_cells(logical_idx, cells);
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
            self.mark_column_layout_dirty();
        }
        self.headers = headers;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::Event;

    #[test]
    fn stale_row_reads_do_not_pollute_current_cache() {
        let mut app = CsvFastViewApp {
            row_request_floor: 1,
            row_request_id: 2,
            ..CsvFastViewApp::default()
        };

        app.apply_event(Event::RowsRead {
            request_id: 1,
            rows: vec![(5, vec![(0, "old".to_string())])],
        });
        assert!(!app.row_cache.contains_key(&5));

        app.apply_event(Event::RowsRead {
            request_id: 2,
            rows: vec![(6, vec![(0, "new".to_string())])],
        });
        assert_eq!(
            app.row_cache.get(&6).and_then(|row| row.get(0)),
            Some("new")
        );
    }
}
