use crate::core::{CsvEncoding, FilterMode, sniff_csv_with_skip};
use crate::worker::Job;
use eframe::egui::{self, RichText, ScrollArea, TextEdit};
use egui_extras::{Column, TableBuilder};

use super::constants::{CURRENT_VIEW_EXPORT_ROWS, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT};
use super::format::{truncate_cell_text, wrap_header_text};
use super::state::CsvFastViewApp;

impl eframe::App for CsvFastViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(evt) = self.worker.rx.try_recv() {
            self.apply_event(evt);
        }

        if let Some(path) = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        }) {
            self.open_path(path);
        }

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
                ui.label(RichText::new("Column filter").strong());
                ui.add(egui::DragValue::new(&mut self.filter_column).range(0..=10_000));
                ui.add(TextEdit::singleline(&mut self.filter_keyword).hint_text("keyword"));
                egui::ComboBox::from_label("Mode")
                    .selected_text(match self.filter_mode {
                        FilterMode::Contains => "contains",
                        FilterMode::Equals => "equals",
                        FilterMode::UniqueByValue => "unique",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.filter_mode,
                            FilterMode::Contains,
                            "contains",
                        );
                        ui.selectable_value(&mut self.filter_mode, FilterMode::Equals, "equals");
                        ui.selectable_value(
                            &mut self.filter_mode,
                            FilterMode::UniqueByValue,
                            "unique",
                        );
                    });

                if ui.button("Apply Filter").clicked() {
                    if self.filtering {
                        self.worker.cancel_filter_now();
                    }
                    self.status = "Filtering in background...".to_string();
                    self.filtering = true;
                    self.filter_progress = None;
                    let _ = self.worker.tx.send(Job::Filter {
                        col: self.filter_column,
                        keyword: self.filter_keyword.clone(),
                        mode: self.filter_mode,
                    });
                }
                if ui
                    .add_enabled(self.filtering, egui::Button::new("Cancel Filter"))
                    .clicked()
                {
                    self.worker.cancel_filter_now();
                }

                if ui.button("Clear Filter").clicked() {
                    self.logical_rows = (0..self.total_rows).collect();
                    self.search_results.clear();
                    self.page_start = 0;
                    self.jump_to = 0;
                    self.scroll_to_row = Some(0);
                    self.clear_rows();
                }
                ui.label("Search");
                ui.add(TextEdit::singleline(&mut self.search_keyword).desired_width(160.0));
                if ui.button("Search All Cols").clicked() {
                    if self.searching {
                        self.worker.cancel_search_now();
                    }
                    self.searching = true;
                    self.search_progress = None;
                    self.status = "Searching in background...".to_string();
                    let _ = self.worker.tx.send(Job::Search {
                        keyword: self.search_keyword.clone(),
                    });
                }
                if ui
                    .add_enabled(self.searching, egui::Button::new("Cancel Search"))
                    .clicked()
                {
                    self.worker.cancel_search_now();
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
                if ui.button("Export Search Results").clicked() {
                    let visible_columns = self.visible_column_indices();
                    self.status = "Exporting search results...".to_string();
                    let _ = self.worker.tx.send(Job::ExportRows {
                        path: self.export_path.clone(),
                        rows: self.search_results.clone(),
                        visible_columns,
                    });
                }
            });
        });

        egui::SidePanel::right("search_results")
            .min_width(220.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Search Results").strong());
                ui.label(format!("count: {}", self.search_results.len()));
                ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    let preview_rows: Vec<usize> =
                        self.search_results.iter().copied().take(200).collect();
                    for row_idx in preview_rows {
                        if ui.button(format!("Row {}", row_idx + 1)).clicked() {
                            if !self.logical_rows.is_empty() {
                                let pos = self
                                    .logical_rows
                                    .iter()
                                    .position(|r| *r == row_idx)
                                    .unwrap_or(
                                        row_idx.min(self.logical_rows.len().saturating_sub(1)),
                                    );
                                self.page_start = pos;
                                self.jump_to = pos;
                                self.scroll_to_row = Some(pos);
                            }
                        }
                    }
                });
            });

        egui::SidePanel::left("columns")
            .min_width(220.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Columns").strong());
                if !self.headers.is_empty() {
                    ScrollArea::both()
                        .id_salt("columns_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, name) in self.headers.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut self.visible_columns[i], "");
                                    ui.add(
                                        egui::Label::new(format!("[{i}] {name}"))
                                            .extend()
                                            .selectable(false),
                                    );
                                });
                            }
                        });
                }
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
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
                if let Some((done, total)) = self.search_progress {
                    ui.separator();
                    let frac = if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.0
                    };
                    ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(180.0));
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
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.headers.is_empty() {
                ui.label("Open a CSV file to start preview.");
                return;
            }
            let headers = self.headers.clone();

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
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(wrap_header_text(&txt)).strong(),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(txt);
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
                                let row_loaded = self.row_cache.contains_key(&row_index);
                                let row_data = self.read_cached_row(row_index);
                                row.col(|ui| {
                                    ui.add(
                                        egui::Label::new((row_index + 1).to_string()).truncate(),
                                    );
                                });
                                for col_idx in &visible {
                                    row.col(|ui| {
                                        if row_loaded {
                                            let val =
                                                row_data.get(*col_idx).cloned().unwrap_or_default();
                                            let shown = truncate_cell_text(&val);
                                            let resp = ui
                                                .add(
                                                    egui::Label::new(shown)
                                                        .truncate()
                                                        .sense(egui::Sense::click()),
                                                )
                                                .on_hover_text(&val);
                                            if resp.clicked() {
                                                self.selected_cell = Some(val);
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

        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}
