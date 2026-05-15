use crate::core::CsvEncoding;
use crate::core::sniff_csv_with_skip;
use crate::worker::Job;
use eframe::egui::{self, RichText, ScrollArea, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::constants::{
    CURRENT_VIEW_EXPORT_ROWS, DELIMITER_INPUT_WIDTH, FILTER_CONTROL_WIDTH, FILTER_PROGRESS_WIDTH,
    FILTER_VALUE_MIN_HEIGHT, FILTER_VALUE_ROW_HEIGHT, SIDEBAR_COLUMN_ROW_HEIGHT, SIDEBAR_MIN_WIDTH,
    STATUS_FILTER_PROGRESS_WIDTH, STATUS_UNIQUE_PROGRESS_WIDTH, TABLE_COLUMN_OVERSCAN_COUNT,
    TABLE_COLUMN_OVERSCAN_WIDTH, TABLE_COLUMN_VIEWPORT_BUFFER_MIN, TABLE_HEADER_HEIGHT,
    TABLE_ROW_HEADER_WIDTH, TABLE_ROW_HEIGHT,
};
use super::state::{ColumnLayoutEntry, CsvFastViewApp};

#[derive(Clone, Copy)]
struct VisibleCol {
    col_idx: usize,
    width: f32,
}

fn visible_column_window(
    column_layout: &[ColumnLayoutEntry],
    total_width: f32,
    viewport_min_x: f32,
    viewport_max_x: f32,
) -> (f32, Vec<VisibleCol>, f32, f32) {
    let mut cols = Vec::new();

    let window_min = (viewport_min_x - TABLE_COLUMN_OVERSCAN_WIDTH).max(TABLE_ROW_HEADER_WIDTH);
    let window_max = viewport_max_x + TABLE_COLUMN_OVERSCAN_WIDTH;
    let mut start_idx = column_layout.partition_point(|col| col.max_x < window_min);
    start_idx = start_idx.saturating_sub(TABLE_COLUMN_OVERSCAN_COUNT);
    let left_spacer = column_layout
        .get(start_idx)
        .map(|entry| entry.min_x - TABLE_ROW_HEADER_WIDTH)
        .unwrap_or(0.0);

    let mut after_window = 0usize;
    for entry in &column_layout[start_idx..] {
        if entry.min_x > window_max {
            if after_window >= TABLE_COLUMN_OVERSCAN_COUNT {
                break;
            }
            after_window += 1;
        }
        cols.push(VisibleCol {
            col_idx: entry.col_idx,
            width: entry.width,
        });
    }

    let shown_width: f32 = cols.iter().map(|col| col.width).sum();
    let right_spacer = (total_width - TABLE_ROW_HEADER_WIDTH - left_spacer - shown_width).max(0.0);

    (left_spacer, cols, right_spacer, total_width)
}

fn contains_filter_value(value: &str, needle: &str, needle_lowercase: &str) -> bool {
    if value.is_ascii() && needle.is_ascii() {
        return contains_ascii_case_insensitive(value.as_bytes(), needle.as_bytes());
    }
    value.to_lowercase().contains(needle_lowercase)
}

fn contains_ascii_case_insensitive(value: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    value
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

impl eframe::App for CsvFastViewApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !raw_input.dropped_files.is_empty() {
            if let Some(path) = raw_input
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
            {
                self.pending_dropped_path = Some(path);
            } else {
                self.status = "Dropped file has no filesystem path".to_string();
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut had_worker_event = false;
        while let Ok(evt) = self.worker.rx.try_recv() {
            had_worker_event = true;
            self.apply_event(evt);
        }

        let dropped_path = self.pending_dropped_path.take().or_else(|| {
            ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .find_map(|file| file.path.clone())
            })
        });
        if let Some(path) = dropped_path {
            self.open_path(path);
        }

        let has_hovered_file = ctx.input(|i| !i.raw.hovered_files.is_empty());

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("File");
                ui.add(TextEdit::singleline(&mut self.path).desired_width(440.0));
                if ui.button("Browse").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV/Text", &["csv", "tsv", "txt"])
                        .pick_file()
                {
                    self.open_path(path);
                }

                ui.label("Delimiter");
                ui.add(
                    TextEdit::singleline(&mut self.delimiter).desired_width(DELIMITER_INPUT_WIDTH),
                );
                ui.label("Quote");
                ui.add(TextEdit::singleline(&mut self.quote).desired_width(DELIMITER_INPUT_WIDTH));
                ui.label("Skip");
                ui.add(egui::DragValue::new(&mut self.skip_lines).range(0..=usize::MAX));
                ui.checkbox(&mut self.has_headers, "Headers");
                ui.checkbox(&mut self.flexible, "Flexible");
                egui::ComboBox::from_label("Encoding")
                    .selected_text(match self.encoding {
                        CsvEncoding::Utf8 => "utf8",
                        CsvEncoding::Gbk => "gbk",
                        CsvEncoding::Gb18030 => "gb18030",
                        CsvEncoding::Big5 => "big5",
                        CsvEncoding::ShiftJis => "shift-jis",
                        CsvEncoding::Iso8859_1 => "iso-8859-1",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.encoding, CsvEncoding::Utf8, "utf8");
                        ui.selectable_value(&mut self.encoding, CsvEncoding::Gbk, "gbk");
                        ui.selectable_value(&mut self.encoding, CsvEncoding::Gb18030, "gb18030");
                        ui.selectable_value(&mut self.encoding, CsvEncoding::Big5, "big5");
                        ui.selectable_value(&mut self.encoding, CsvEncoding::ShiftJis, "shift-jis");
                        ui.selectable_value(
                            &mut self.encoding,
                            CsvEncoding::Iso8859_1,
                            "iso-8859-1",
                        );
                    });

                if ui.button("Auto Detect").clicked() {
                    match sniff_csv_with_skip(&self.path, self.skip_lines) {
                        Ok(s) => {
                            self.delimiter = (s.delimiter as char).to_string();
                            self.has_headers = s.has_headers;
                            self.encoding = s.encoding;
                            self.status = "Auto detect done".to_string();
                        }
                        Err(e) => self.status = format!("Auto detect failed: {e}"),
                    }
                }

                if ui.button("Open").clicked() {
                    self.open_current_file();
                }
            });

            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.has_selected_unique_filters(),
                        egui::Button::new("Apply Filters"),
                    )
                    .clicked()
                {
                    if self.filtering {
                        self.worker.cancel_query_now();
                    }
                    self.status = "Applying filters in background...".to_string();
                    self.filtering = true;
                    self.filter_progress = None;
                    let _ = self.worker.tx.send(Job::ApplyUniqueFilters {
                        filters: self.selected_unique_filters(),
                    });
                }
                if ui
                    .add_enabled(
                        self.filtering || self.unique_columns.values().any(|s| s.indexing),
                        egui::Button::new("Cancel"),
                    )
                    .clicked()
                {
                    self.worker.cancel_query_now();
                }

                if ui.button("Clear Filters").clicked() {
                    for state in self.unique_columns.values_mut() {
                        state.selected.clear();
                        state.excluded.clear();
                        state.all_selected = false;
                    }
                    self.set_all_rows(self.total_rows);
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                }

                ui.label("Export");
                ui.add(
                    TextEdit::singleline(&mut self.export_path).desired_width(FILTER_CONTROL_WIDTH),
                );
                if ui.button("Export Current View").clicked() {
                    let end =
                        (self.page_start + CURRENT_VIEW_EXPORT_ROWS).min(self.logical_rows.len());
                    let rows = self.logical_rows.slice_to_vec(self.page_start, end);
                    let visible_columns = self.visible_column_indices();
                    self.status = "Exporting current view...".to_string();
                    let _ = self.worker.tx.send(Job::ExportRows {
                        path: self.export_path.clone(),
                        rows,
                        visible_columns,
                    });
                }
            });
        });

        egui::SidePanel::left("columns")
            .min_width(SIDEBAR_MIN_WIDTH)
            .show(ctx, |ui| {
                let split_gap = ui.spacing().item_spacing.y;
                let half_height = (ui.available_height() - split_gap).max(0.0) * 0.5;
                ui.allocate_ui(egui::vec2(ui.available_width(), half_height), |ui| {
                    ui.label(RichText::new("Columns").strong());
                    let row_height = SIDEBAR_COLUMN_ROW_HEIGHT;
                    let row_count = self.headers.len();
                    let content_width = 900.0_f32.max(ui.available_width());
                    if row_count == 0 {
                        ui.label("No columns");
                    } else {
                        ScrollArea::both()
                            .id_salt("columns_scroll")
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, row_count, |ui, row_range| {
                                ui.set_min_width(content_width);
                                for i in row_range {
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(content_width);
                                        if ui.checkbox(&mut self.visible_columns[i], "").changed() {
                                            self.mark_column_layout_dirty();
                                        }
                                        ui.label(format!("[{i}]"));
                                        let response = ui.add(
                                            egui::Label::new(self.headers[i].as_str())
                                                .extend()
                                                .selectable(false),
                                        );
                                        response.context_menu(|ui| {
                                            if ui.button("Index unique values").clicked() {
                                                let state =
                                                    self.unique_columns.entry(i).or_default();
                                                state.indexing = true;
                                                state.progress = None;
                                                state.error = None;
                                                self.active_filter_column = Some(i);
                                                self.status = format!(
                                                    "Indexing unique values for column {i}..."
                                                );
                                                let _ = self
                                                    .worker
                                                    .tx
                                                    .send(Job::IndexUnique { col: i });
                                                ui.close_menu();
                                            }
                                            if ui.button("Show filter values").clicked() {
                                                self.active_filter_column = Some(i);
                                                ui.close_menu();
                                            }
                                        });
                                    });
                                }
                            });
                    }
                });

                ui.separator();

                ui.allocate_ui(
                    egui::vec2(ui.available_width(), ui.available_height()),
                    |ui| {
                        ui.label(RichText::new("Filters").strong());
                        ui.label(format!("shown rows: {}", self.logical_rows.len()));

                        let mut indexed: Vec<usize> = self.unique_columns.keys().copied().collect();
                        indexed.sort_unstable();
                        if indexed.is_empty() {
                            ui.label("Right-click a column and index unique values.");
                            return;
                        }

                        egui::ComboBox::from_label("Column")
                            .selected_text(
                                self.active_filter_column
                                    .and_then(|col| self.headers.get(col).cloned())
                                    .unwrap_or_else(|| "Select indexed column".to_string()),
                            )
                            .show_ui(ui, |ui| {
                                for col in indexed {
                                    let name = self
                                        .headers
                                        .get(col)
                                        .cloned()
                                        .unwrap_or_else(|| format!("Column {}", col + 1));
                                    ui.selectable_value(
                                        &mut self.active_filter_column,
                                        Some(col),
                                        format!("[{col}] {name}"),
                                    );
                                }
                            });

                        let Some(col) = self.active_filter_column else {
                            return;
                        };
                        let Some(state) = self.unique_columns.get_mut(&col) else {
                            return;
                        };

                        if let Some((done, total)) = state.progress {
                            let frac = if total > 0 {
                                done as f32 / total as f32
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(frac.clamp(0.0, 1.0))
                                    .desired_width(FILTER_PROGRESS_WIDTH),
                            );
                        }
                        if let Some(err) = &state.error {
                            ui.label(RichText::new(err).color(egui::Color32::LIGHT_RED));
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("All").clicked() {
                                state.selected.clear();
                                state.excluded.clear();
                                state.all_selected = true;
                            }
                            if ui.small_button("None").clicked() {
                                state.selected.clear();
                                state.excluded.clear();
                                state.all_selected = false;
                            }
                            let selected_count = if state.all_selected {
                                state.values.len().saturating_sub(state.excluded.len())
                            } else {
                                state.selected.len()
                            };
                            ui.label(format!(
                                "{} selected / {} values",
                                selected_count,
                                state.values.len()
                            ));
                        });

                        ui.add(
                            TextEdit::singleline(&mut state.value_filter)
                                .hint_text("filter values")
                                .desired_width(FILTER_CONTROL_WIDTH),
                        );

                        let filter_is_empty = state.value_filter.is_empty();
                        if filter_is_empty {
                            state.cached_value_filter.clear();
                            state.cached_filter_value_count = state.values.len();
                            state.cached_filtered_indices.clear();
                        } else if state.cached_value_filter != state.value_filter
                            || state.cached_filter_value_count != state.values.len()
                        {
                            let needle = state.value_filter.as_str();
                            let needle_lowercase = state.value_filter.to_lowercase();
                            state.cached_filtered_indices = state
                                .values
                                .iter()
                                .enumerate()
                                .filter_map(|(idx, item)| {
                                    contains_filter_value(&item.value, needle, &needle_lowercase)
                                        .then_some(idx)
                                })
                                .collect();
                            state.cached_value_filter = state.value_filter.clone();
                            state.cached_filter_value_count = state.values.len();
                        }
                        let filtered_len = if filter_is_empty {
                            state.values.len()
                        } else {
                            state.cached_filtered_indices.len()
                        };

                        ui.label(format!("visible values: {}", filtered_len));
                        let bottom_padding =
                            ui.spacing().scroll.allocated_width() + ui.spacing().item_spacing.y;
                        let values_height =
                            (ui.available_height() - bottom_padding).max(FILTER_VALUE_MIN_HEIGHT);
                        ScrollArea::vertical()
                            .id_salt(("unique_values", col))
                            .auto_shrink([false, false])
                            .max_height(values_height)
                            .show_rows(
                                ui,
                                FILTER_VALUE_ROW_HEIGHT,
                                filtered_len,
                                |ui, row_range| {
                                    for row in row_range {
                                        let value_idx = if filter_is_empty {
                                            row
                                        } else {
                                            state.cached_filtered_indices[row]
                                        };
                                        let item = &state.values[value_idx];
                                        let mut checked = if state.all_selected {
                                            !state.excluded.contains(&item.value)
                                        } else {
                                            state.selected.contains(&item.value)
                                        };
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut checked, "").changed() {
                                                if state.all_selected {
                                                    if checked {
                                                        state.excluded.remove(&item.value);
                                                    } else {
                                                        state.excluded.insert(item.value.clone());
                                                    }
                                                } else if checked {
                                                    state.selected.insert(item.value.clone());
                                                } else {
                                                    state.selected.remove(&item.value);
                                                }
                                            }
                                            ui.add(
                                                egui::Label::new(format!(
                                                    "{} ({})",
                                                    item.value, item.count
                                                ))
                                                .truncate(),
                                            )
                                            .on_hover_text(&item.value);
                                        });
                                    }
                                },
                            );
                    },
                );
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            self.render_status_bar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.headers.is_empty() {
                ui.label("Open a CSV file to start preview.");
                return;
            }
            let bottom_reserved_height =
                ui.spacing().scroll.allocated_width() + ui.spacing().item_spacing.y * 2.0;
            let table_body_height =
                (ui.available_height() - TABLE_HEADER_HEIGHT - bottom_reserved_height)
                    .max(TABLE_ROW_HEIGHT);
            let table_view_width = ui.available_width();
            ScrollArea::horizontal()
                .id_salt("csv_horizontal_scroll")
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    self.rebuild_column_layout_if_needed();
                    let viewport_buffer = table_view_width.max(TABLE_COLUMN_VIEWPORT_BUFFER_MIN);
                    let (left_spacer, visible_cols, right_spacer, total_width) =
                        visible_column_window(
                            &self.column_layout,
                            self.table_total_width,
                            (viewport.min.x - viewport_buffer).max(0.0),
                            viewport.max.x + viewport_buffer,
                        );
                    self.rendered_table_columns = visible_cols.len();
                    let table_width = total_width.max(table_view_width);
                    ui.set_min_width(table_width);

                    let mut table = TableBuilder::new(ui)
                        .id_salt("csv_table")
                        .striped(true)
                        .resizable(false)
                        .auto_shrink([false, false])
                        .min_scrolled_height(table_body_height)
                        .max_scroll_height(table_body_height);

                    table = table.column(Column::exact(TABLE_ROW_HEADER_WIDTH).clip(true));
                    if left_spacer > 0.0 {
                        table = table.column(Column::exact(left_spacer).clip(true));
                    }
                    for col in &visible_cols {
                        table = table.column(Column::exact(col.width).clip(true));
                    }
                    if right_spacer > 0.0 {
                        table = table.column(Column::exact(right_spacer).clip(true));
                    }
                    if let Some(row) = self.scroll_to_row.take() {
                        table = table.scroll_to_row(row, Some(egui::Align::TOP));
                    }

                    let rendered_col_indices: Vec<usize> =
                        visible_cols.iter().map(|col| col.col_idx).collect();
                    let mut selected_cell = None;

                    table
                        .header(TABLE_HEADER_HEIGHT, |mut header| {
                            header.col(|ui| {
                                ui.add(egui::Label::new(RichText::new("#").strong()).truncate());
                            });
                            if left_spacer > 0.0 {
                                header.col(|ui| {
                                    ui.allocate_space(egui::vec2(left_spacer, TABLE_HEADER_HEIGHT));
                                });
                            }
                            for col in &visible_cols {
                                let col_idx = col.col_idx;
                                let txt = self.headers.get(col_idx).map_or("", String::as_str);
                                header.col(|ui| {
                                    let response = ui
                                        .add(
                                            egui::Label::new(RichText::new(txt).strong())
                                                .truncate(),
                                        )
                                        .on_hover_text(txt);
                                    response.context_menu(|ui| {
                                        if ui.button("Index unique values").clicked() {
                                            let state =
                                                self.unique_columns.entry(col_idx).or_default();
                                            state.indexing = true;
                                            state.progress = None;
                                            state.error = None;
                                            self.active_filter_column = Some(col_idx);
                                            self.status = format!(
                                                "Indexing unique values for column {}...",
                                                col_idx
                                            );
                                            let _ = self
                                                .worker
                                                .tx
                                                .send(Job::IndexUnique { col: col_idx });
                                            ui.close_menu();
                                        }
                                        if ui.button("Show filter values").clicked() {
                                            self.active_filter_column = Some(col_idx);
                                            ui.close_menu();
                                        }
                                    });
                                });
                            }
                            if right_spacer > 0.0 {
                                header.col(|ui| {
                                    ui.allocate_space(egui::vec2(
                                        right_spacer,
                                        TABLE_HEADER_HEIGHT,
                                    ));
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(TABLE_ROW_HEIGHT, self.logical_rows.len(), |mut row| {
                                let row_index = row.index();
                                self.page_start = row_index;
                                self.request_cached_row(row_index, &rendered_col_indices);
                                let row_data = self.row_cache.get(&row_index);
                                row.col(|ui| {
                                    ui.add(
                                        egui::Label::new((row_index + 1).to_string()).truncate(),
                                    );
                                });
                                if left_spacer > 0.0 {
                                    row.col(|ui| {
                                        ui.allocate_space(egui::vec2(
                                            left_spacer,
                                            TABLE_ROW_HEIGHT,
                                        ));
                                    });
                                }
                                for col in &visible_cols {
                                    let col_idx = col.col_idx;
                                    row.col(|ui| {
                                        if let Some(row_data) = row_data {
                                            let val = row_data.get(col_idx).unwrap_or("");
                                            let resp = ui.add(
                                                egui::Label::new(val)
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                            );
                                            if resp.hovered() {
                                                resp.clone().on_hover_text(val);
                                            }
                                            if resp.clicked() {
                                                selected_cell = Some(val.to_string());
                                            }
                                        } else {
                                            ui.add(
                                                egui::Label::new(RichText::new("...").weak())
                                                    .truncate(),
                                            );
                                        }
                                    });
                                }
                                if right_spacer > 0.0 {
                                    row.col(|ui| {
                                        ui.allocate_space(egui::vec2(
                                            right_spacer,
                                            TABLE_ROW_HEIGHT,
                                        ));
                                    });
                                }
                            });
                        });
                    if selected_cell.is_some() {
                        self.selected_cell = selected_cell;
                    }
                });
        });

        if self.selected_cell.is_some() {
            let mut open = true;
            egui::Window::new("Cell Full Content")
                .open(&mut open)
                .show(ctx, |ui| {
                    if let Some(val) = self.selected_cell.as_mut() {
                        ui.add(
                            TextEdit::multiline(val)
                                .desired_rows(12)
                                .desired_width(560.0),
                        );
                    }
                });
            if !open {
                self.selected_cell = None;
            }
        }

        if has_hovered_file {
            egui::Area::new("drop_file_overlay".into())
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(RichText::new("Drop file to open").strong());
                    });
                });
        }

        if self.indexing
            || self.filtering
            || self.unique_columns.values().any(|state| state.indexing)
            || self.requested_range.is_some()
            || has_hovered_file
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        } else if had_worker_event {
            ctx.request_repaint();
        }
    }
}

impl CsvFastViewApp {
    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let total = self.logical_rows.len();
        ui.horizontal_wrapped(|ui| {
            ui.label(&self.status);
            if let Some((done, total)) = self.filter_progress {
                ui.separator();
                let frac = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac.clamp(0.0, 1.0))
                        .desired_width(STATUS_FILTER_PROGRESS_WIDTH),
                );
            }
            for (col, state) in &self.unique_columns {
                if let Some((done, total)) = state.progress {
                    ui.separator();
                    let frac = if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.0
                    };
                    ui.label(format!("unique col {col}"));
                    ui.add(
                        egui::ProgressBar::new(frac.clamp(0.0, 1.0))
                            .desired_width(STATUS_UNIQUE_PROGRESS_WIDTH),
                    );
                }
            }
            ui.separator();
            ui.label(format!("file_size: {}", self.file_size_text));
            ui.separator();
            ui.label(format!("rows: {total}"));
            ui.separator();
            ui.label(format!("cols: {}", self.rendered_table_columns));
            ui.separator();
            ui.label(format!("row: {}", self.page_start + 1));

            ui.label("jump");
            ui.add(egui::DragValue::new(&mut self.jump_to).range(0..=usize::MAX));
            if ui.button("Go").clicked() {
                self.page_start = self.jump_to.min(total.saturating_sub(1));
                self.jump_to = self.page_start;
                self.scroll_to_row = Some(self.page_start);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_filter_match_is_case_insensitive_without_lowercase_allocations() {
        assert!(contains_filter_value("ESP01", "esp", "esp"));
        assert!(contains_filter_value("sensor_VALUE", "VALUE", "value"));
        assert!(!contains_filter_value("ESP01", "ic", "ic"));
    }

    #[test]
    fn unicode_filter_match_still_works() {
        assert!(contains_filter_value("城市-上海", "上海", "上海"));
    }
}
