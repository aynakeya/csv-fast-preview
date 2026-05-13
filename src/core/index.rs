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
        let mut file = File::open(&self.path)?;
        for logical_idx in start..end {
            let real_row = source_rows[logical_idx];
            if let Some(row) = self.read_record_by_row_with_file(&mut file, real_row)? {
                rows.push(row);
            }
        }
        Ok(rows)
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
