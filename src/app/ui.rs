use crate::core::CsvEncoding;
use crate::core::sniff_csv_with_skip;
use crate::worker::Job;
use eframe::egui::{self, RichText, ScrollArea, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::constants::{CURRENT_VIEW_EXPORT_ROWS, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT};
use super::format::wrap_header_text;
use super::state::CsvFastViewApp;

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
                if ui.button("Browse").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV/Text", &["csv", "tsv", "txt"])
                        .pick_file()
                    {
                        self.open_path(path);
                    }
                }

                ui.label("Delimiter");
                ui.add(TextEdit::singleline(&mut self.delimiter).desired_width(24.0));
                ui.label("Quote");
                ui.add(TextEdit::singleline(&mut self.quote).desired_width(24.0));
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
                    }
                    self.logical_rows = (0..self.total_rows).collect();
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                }

                ui.label("Export");
                ui.add(TextEdit::singleline(&mut self.export_path).desired_width(260.0));
                if ui.button("Export Current View").clicked() {
                    let end =
                        (self.page_start + CURRENT_VIEW_EXPORT_ROWS).min(self.logical_rows.len());
                    let rows = self.logical_rows[self.page_start..end].to_vec();
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
            .min_width(300.0)
            .show(ctx, |ui| {
                let split_gap = ui.spacing().item_spacing.y;
                let half_height = (ui.available_height() - split_gap).max(0.0) * 0.5;
                ui.allocate_ui(egui::vec2(ui.available_width(), half_height), |ui| {
                    ui.label(RichText::new("Columns").strong());
                    let row_height = 22.0;
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
                                    let name = self.headers[i].clone();
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(content_width);
                                        ui.checkbox(&mut self.visible_columns[i], "");
                                        let response = ui.add(
                                            egui::Label::new(format!("[{i}] {name}"))
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
                                egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(240.0),
                            );
                        }
                        if let Some(err) = &state.error {
                            ui.label(RichText::new(err).color(egui::Color32::LIGHT_RED));
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("All").clicked() {
                                state.selected.clear();
                                state
                                    .selected
                                    .extend(state.values.iter().map(|item| item.value.clone()));
                            }
                            if ui.small_button("None").clicked() {
                                state.selected.clear();
                            }
                            ui.label(format!(
                                "{} selected / {} values",
                                state.selected.len(),
                                state.values.len()
                            ));
                        });

                        ui.add(
                            TextEdit::singleline(&mut state.value_filter)
                                .hint_text("filter values")
                                .desired_width(260.0),
                        );

                        let filtered_indices: Vec<usize> = if state.value_filter.is_empty() {
                            (0..state.values.len()).collect()
                        } else {
                            let needle = state.value_filter.to_lowercase();
                            state
                                .values
                                .iter()
                                .enumerate()
                                .filter_map(|(idx, item)| {
                                    if item.value.to_lowercase().contains(&needle) {
                                        Some(idx)
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        };

                        ui.label(format!("visible values: {}", filtered_indices.len()));
                        let bottom_padding =
                            ui.spacing().scroll.allocated_width() + ui.spacing().item_spacing.y;
                        let values_height = (ui.available_height() - bottom_padding).max(24.0);
                        ScrollArea::vertical()
                            .id_salt(("unique_values", col))
                            .auto_shrink([false, false])
                            .max_height(values_height)
                            .show_rows(ui, 20.0, filtered_indices.len(), |ui, row_range| {
                                for row in row_range {
                                    let item = &state.values[filtered_indices[row]];
                                    let mut checked = state.selected.contains(&item.value);
                                    ui.horizontal(|ui| {
                                        if ui.checkbox(&mut checked, "").changed() {
                                            if checked {
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
                            });
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
            let visible: Vec<usize> = self
                .visible_columns
                .iter()
                .enumerate()
                .filter_map(|(i, v)| if *v { Some(i) } else { None })
                .collect();

            let bottom_reserved_height =
                ui.spacing().scroll.allocated_width() + ui.spacing().item_spacing.y * 2.0;
            let table_body_height =
                (ui.available_height() - TABLE_HEADER_HEIGHT - bottom_reserved_height)
                    .max(TABLE_ROW_HEIGHT);
            let table_width = (72.0
                + visible
                    .iter()
                    .map(|col_idx| self.column_widths[*col_idx])
                    .sum::<f32>())
            .max(ui.available_width());
            ScrollArea::horizontal()
                .id_salt("csv_horizontal_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(table_width);
                    let mut table = TableBuilder::new(ui)
                        .id_salt("csv_table")
                        .striped(true)
                        .resizable(true)
                        .auto_shrink([false, false])
                        .min_scrolled_height(table_body_height)
                        .max_scroll_height(table_body_height);

                    table = table.column(Column::initial(72.0).resizable(true).clip(true));
                    for col_idx in &visible {
                        let w = self.column_widths[*col_idx];
                        table = table.column(Column::initial(w).resizable(true).clip(true));
                    }
                    if visible.is_empty() {
                        table = table.column(Column::remainder());
                    }
                    if let Some(row) = self.scroll_to_row.take() {
                        table = table.scroll_to_row(row, Some(egui::Align::TOP));
                    }

                    let headers = self.headers.clone();
                    let mut selected_cell = None;

                    table
                        .header(TABLE_HEADER_HEIGHT, |mut header| {
                            header.col(|ui| {
                                ui.add(egui::Label::new(RichText::new("#").strong()).truncate());
                            });
                            for col_idx in &visible {
                                let txt = headers
                                    .get(*col_idx)
                                    .cloned()
                                    .unwrap_or_else(|| format!("Column {}", col_idx + 1));
                                header.col(|ui| {
                                    let response = ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(wrap_header_text(&txt)).strong(),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(txt);
                                    response.context_menu(|ui| {
                                        if ui.button("Index unique values").clicked() {
                                            let state =
                                                self.unique_columns.entry(*col_idx).or_default();
                                            state.indexing = true;
                                            state.progress = None;
                                            state.error = None;
                                            self.active_filter_column = Some(*col_idx);
                                            self.status = format!(
                                                "Indexing unique values for column {}...",
                                                col_idx
                                            );
                                            let _ = self
                                                .worker
                                                .tx
                                                .send(Job::IndexUnique { col: *col_idx });
                                            ui.close_menu();
                                        }
                                        if ui.button("Show filter values").clicked() {
                                            self.active_filter_column = Some(*col_idx);
                                            ui.close_menu();
                                        }
                                    });
                                });
                            }
                            if visible.is_empty() {
                                header.col(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new("No visible columns").strong(),
                                        )
                                        .truncate(),
                                    );
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(TABLE_ROW_HEIGHT, self.logical_rows.len(), |mut row| {
                                let row_index = row.index();
                                self.page_start = row_index;
                                self.request_cached_row(row_index);
                                let row_data = self.row_cache.get(&row_index);
                                row.col(|ui| {
                                    ui.add(
                                        egui::Label::new((row_index + 1).to_string()).truncate(),
                                    );
                                });
                                for col_idx in &visible {
                                    row.col(|ui| {
                                        if let Some(row_data) = row_data {
                                            let val = row_data
                                                .get(*col_idx)
                                                .map(String::as_str)
                                                .unwrap_or("");
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
                                if visible.is_empty() {
                                    row.col(|ui| {
                                        ui.label("");
                                    });
                                }
                            });
                        });
                    if selected_cell.is_some() {
                        self.selected_cell = selected_cell;
                    }
                });
        });

        if let Some(val) = self.selected_cell.clone() {
            let mut open = true;
            egui::Window::new("Cell Full Content")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.add(
                        TextEdit::multiline(&mut val.clone())
                            .desired_rows(12)
                            .desired_width(560.0),
                    );
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
                ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(180.0));
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
                    ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(120.0));
                }
            }
            ui.separator();
            ui.label(format!("file_size: {}", self.file_size_text));
            ui.separator();
            ui.label(format!("rows: {total}"));
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
