use anyhow::{Context, Result};
use csv::ByteRecord;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::config::CsvConfig;

#[derive(Clone, Debug)]
pub struct CsvIndex {
    pub path: PathBuf,
    pub config: CsvConfig,
    pub headers: Vec<String>,
    pub row_offsets: Vec<u64>,
}

pub struct CsvReadPlan {
    path: PathBuf,
    config: CsvConfig,
    rows: Vec<PlannedRow>,
}

#[derive(Clone, Copy)]
struct PlannedRow {
    row_idx: usize,
    offset: u64,
}

impl CsvIndex {
    pub fn preview(path: impl AsRef<Path>, config: CsvConfig, limit: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut rdr = config.build_reader(file, false);
        let mut record = ByteRecord::new();
        let mut headers = prepare_headers(&mut rdr, &config, &mut record)?;

        let mut row_offsets = Vec::new();
        while row_offsets.len() < limit && !rdr.is_done() {
            let start = rdr.position().byte();
            if !rdr
                .read_byte_record(&mut record)
                .context("read CSV preview record")?
            {
                break;
            }
            row_offsets.push(start);
            if headers.is_empty() {
                headers = (0..record.len())
                    .map(|i| format!("Column {}", i + 1))
                    .collect();
            }
        }

        Ok(Self {
            path,
            config,
            headers,
            row_offsets,
        })
    }

    pub fn build(path: impl AsRef<Path>, config: CsvConfig) -> Result<Self> {
        Self::build_with_progress(path, config, |_, _, _, _| {})
    }

    pub fn build_with_progress<F>(
        path: impl AsRef<Path>,
        config: CsvConfig,
        mut on_progress: F,
    ) -> Result<Self>
    where
        F: FnMut(Vec<u64>, usize, u64, u64),
    {
        let path = path.as_ref().to_path_buf();
        let total_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut rdr = config.build_reader(file, false);
        let mut record = ByteRecord::new();
        let mut headers = prepare_headers(&mut rdr, &config, &mut record)?;

        let mut row_offsets = Vec::new();
        let mut progress_chunk = Vec::new();
        while !rdr.is_done() {
            let start = rdr.position().byte();
            if !rdr
                .read_byte_record(&mut record)
                .context("read CSV record")?
            {
                break;
            }
            row_offsets.push(start);
            progress_chunk.push(start);
            if headers.is_empty() {
                headers = (0..record.len())
                    .map(|i| format!("Column {}", i + 1))
                    .collect();
            }
            let indexed_rows = row_offsets.len();
            let byte_pos = rdr.position().byte();
            if indexed_rows == 200 || progress_chunk.len() >= 10_000 {
                on_progress(
                    std::mem::take(&mut progress_chunk),
                    indexed_rows,
                    byte_pos,
                    total_bytes,
                );
            }
        }

        if !progress_chunk.is_empty() {
            let indexed_rows = row_offsets.len();
            let byte_pos = rdr.position().byte();
            on_progress(progress_chunk, indexed_rows, byte_pos, total_bytes);
        }

        Ok(Self {
            path,
            config,
            headers,
            row_offsets,
        })
    }

    pub fn read_page(
        &self,
        source_rows: &[usize],
        start: usize,
        page_size: usize,
    ) -> Result<Vec<Vec<String>>> {
        let mut rows = Vec::new();
        let end = (start + page_size).min(source_rows.len());
        let requested_rows = &source_rows[start..end];
        let mut file = File::open(&self.path)?;
        for run in consecutive_runs(requested_rows) {
            if run.len() > 1 {
                rows.extend(
                    self.read_consecutive_rows_with_file(&mut file, run, |rec, out| {
                        out.push(
                            rec.iter()
                                .map(|field| self.config.decode_field(field))
                                .collect(),
                        );
                    })?,
                );
            } else if let Some(row) = self.read_record_by_row_with_file(&mut file, run[0])? {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    pub fn read_page_columns(
        &self,
        source_rows: &[usize],
        columns: &[usize],
        start: usize,
        page_size: usize,
    ) -> Result<Vec<Vec<(usize, String)>>> {
        let mut rows = Vec::new();
        let end = (start + page_size).min(source_rows.len());
        let requested_rows = &source_rows[start..end];
        let mut file = File::open(&self.path)?;
        for run in consecutive_runs(requested_rows) {
            if run.len() > 1 {
                rows.extend(
                    self.read_consecutive_rows_with_file(&mut file, run, |rec, out| {
                        out.push(decode_selected_cells(&self.config, rec, columns));
                    })?,
                );
            } else if let Some(row) =
                self.read_record_columns_by_row_with_file(&mut file, run[0], columns)?
            {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    pub fn read_plan(&self, source_rows: &[usize]) -> CsvReadPlan {
        let rows = source_rows
            .iter()
            .filter_map(|row_idx| {
                self.row_offsets
                    .get(*row_idx)
                    .copied()
                    .map(|offset| PlannedRow {
                        row_idx: *row_idx,
                        offset,
                    })
            })
            .collect();
        CsvReadPlan {
            path: self.path.clone(),
            config: self.config.clone(),
            rows,
        }
    }

    pub fn all_rows(&self) -> Vec<usize> {
        (0..self.row_offsets.len()).collect()
    }

    fn read_record_by_row_with_file(
        &self,
        file: &mut File,
        row_idx: usize,
    ) -> Result<Option<Vec<String>>> {
        let Some(offset) = self.row_offsets.get(row_idx).copied() else {
            return Ok(None);
        };

        file.seek(SeekFrom::Start(offset))?;

        let mut rdr = self
            .config
            .build_reader(OneRecordReader { inner: file }, false);
        let mut rec = ByteRecord::new();
        if rdr.read_byte_record(&mut rec)? {
            Ok(Some(
                rec.iter()
                    .map(|field| self.config.decode_field(field))
                    .collect(),
            ))
        } else {
            Ok(None)
        }
    }

    fn read_record_columns_by_row_with_file(
        &self,
        file: &mut File,
        row_idx: usize,
        columns: &[usize],
    ) -> Result<Option<Vec<(usize, String)>>> {
        let Some(offset) = self.row_offsets.get(row_idx).copied() else {
            return Ok(None);
        };

        file.seek(SeekFrom::Start(offset))?;

        let mut rdr = self
            .config
            .build_reader(OneRecordReader { inner: file }, false);
        let mut rec = ByteRecord::new();
        if !rdr.read_byte_record(&mut rec)? {
            return Ok(None);
        }

        Ok(Some(decode_selected_cells(&self.config, &rec, columns)))
    }

    fn read_consecutive_rows_with_file<T, F>(
        &self,
        file: &mut File,
        source_rows: &[usize],
        mut decode: F,
    ) -> Result<Vec<T>>
    where
        F: FnMut(&ByteRecord, &mut Vec<T>),
    {
        let Some(first_row) = source_rows.first().copied() else {
            return Ok(Vec::new());
        };
        let Some(offset) = self.row_offsets.get(first_row).copied() else {
            return Ok(Vec::new());
        };

        file.seek(SeekFrom::Start(offset))?;

        let mut rdr = self
            .config
            .build_reader(OneRecordReader { inner: file }, false);
        let mut rec = ByteRecord::new();
        let mut rows = Vec::with_capacity(source_rows.len());
        for _ in source_rows {
            if !rdr.read_byte_record(&mut rec)? {
                break;
            }
            decode(&rec, &mut rows);
        }
        Ok(rows)
    }
}

impl CsvReadPlan {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_columns(&self, columns: &[usize]) -> Result<Vec<Vec<(usize, String)>>> {
        let mut file = File::open(&self.path)?;
        self.read_columns_range_with_file(&mut file, columns, 0, self.rows.len())
    }

    pub fn open_file(&self) -> Result<File> {
        File::open(&self.path).with_context(|| format!("open {}", self.path.display()))
    }

    pub fn read_columns_range_with_file(
        &self,
        file: &mut File,
        columns: &[usize],
        start: usize,
        end: usize,
    ) -> Result<Vec<Vec<(usize, String)>>> {
        let mut rows = Vec::new();
        let end = end.min(self.rows.len());
        if start >= end {
            return Ok(rows);
        }
        for run in planned_consecutive_runs(&self.rows[start..end]) {
            if run.len() > 1 {
                rows.extend(self.read_consecutive_columns_with_file(file, run, columns)?);
            } else if let Some(row) =
                self.read_record_columns_with_file(file, run[0].offset, columns)?
            {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    fn read_record_columns_with_file(
        &self,
        file: &mut File,
        offset: u64,
        columns: &[usize],
    ) -> Result<Option<Vec<(usize, String)>>> {
        file.seek(SeekFrom::Start(offset))?;

        let mut rdr = self
            .config
            .build_reader(OneRecordReader { inner: file }, false);
        let mut rec = ByteRecord::new();
        if !rdr.read_byte_record(&mut rec)? {
            return Ok(None);
        }

        Ok(Some(decode_selected_cells(&self.config, &rec, columns)))
    }

    fn read_consecutive_columns_with_file(
        &self,
        file: &mut File,
        rows: &[PlannedRow],
        columns: &[usize],
    ) -> Result<Vec<Vec<(usize, String)>>> {
        let Some(first_row) = rows.first().copied() else {
            return Ok(Vec::new());
        };

        file.seek(SeekFrom::Start(first_row.offset))?;

        let mut rdr = self
            .config
            .build_reader(OneRecordReader { inner: file }, false);
        let mut rec = ByteRecord::new();
        let mut out = Vec::with_capacity(rows.len());
        for _ in rows {
            if !rdr.read_byte_record(&mut rec)? {
                break;
            }
            out.push(decode_selected_cells(&self.config, &rec, columns));
        }
        Ok(out)
    }
}

fn consecutive_runs(rows: &[usize]) -> ConsecutiveRuns<'_> {
    ConsecutiveRuns { rows, start: 0 }
}

fn planned_consecutive_runs(rows: &[PlannedRow]) -> PlannedConsecutiveRuns<'_> {
    PlannedConsecutiveRuns { rows, start: 0 }
}

struct ConsecutiveRuns<'a> {
    rows: &'a [usize],
    start: usize,
}

impl<'a> Iterator for ConsecutiveRuns<'a> {
    type Item = &'a [usize];

    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.rows.len() {
            return None;
        }

        let run_start = self.start;
        self.start += 1;
        while self.start < self.rows.len() && self.rows[self.start] == self.rows[self.start - 1] + 1
        {
            self.start += 1;
        }
        Some(&self.rows[run_start..self.start])
    }
}

struct PlannedConsecutiveRuns<'a> {
    rows: &'a [PlannedRow],
    start: usize,
}

impl<'a> Iterator for PlannedConsecutiveRuns<'a> {
    type Item = &'a [PlannedRow];

    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.rows.len() {
            return None;
        }

        let run_start = self.start;
        self.start += 1;
        while self.start < self.rows.len()
            && self.rows[self.start].row_idx == self.rows[self.start - 1].row_idx + 1
        {
            self.start += 1;
        }
        Some(&self.rows[run_start..self.start])
    }
}

fn decode_selected_cells(
    config: &CsvConfig,
    rec: &ByteRecord,
    columns: &[usize],
) -> Vec<(usize, String)> {
    let mut cells = Vec::with_capacity(columns.len());
    for &col in columns {
        let value = rec
            .get(col)
            .map(|field| config.decode_field(field))
            .unwrap_or_default();
        cells.push((col, value));
    }
    cells
}

struct OneRecordReader<'a> {
    inner: &'a mut File,
}

impl Read for OneRecordReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

fn prepare_headers(
    rdr: &mut csv::Reader<File>,
    config: &CsvConfig,
    record: &mut ByteRecord,
) -> Result<Vec<String>> {
    for _ in 0..config.skip_lines {
        if !rdr
            .read_byte_record(record)
            .context("skip configured CSV record")?
        {
            return Ok(Vec::new());
        }
    }

    if config.has_headers && rdr.read_byte_record(record).context("read CSV headers")? {
        Ok(record.iter().map(|f| config.decode_field(f)).collect())
    } else {
        Ok(Vec::new())
    }
}
