use anyhow::{Context, Result};
use csv::ByteRecord;
use std::collections::HashSet;
use std::fs::File;

use super::index::CsvIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    Contains,
    Equals,
    UniqueByValue,
}

impl CsvIndex {
    pub fn filter_rows(&self, col: usize, keyword: &str, mode: FilterMode) -> Result<Vec<usize>> {
        self.filter_rows_with_progress(col, keyword, mode, |_, _| false)
    }

    pub fn filter_rows_with_progress<F>(
        &self,
        col: usize,
        keyword: &str,
        mode: FilterMode,
        mut on_progress: F,
    ) -> Result<Vec<usize>>
    where
        F: FnMut(usize, usize) -> bool,
    {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let file =
            File::open(&self.path).with_context(|| format!("open {}", self.path.display()))?;
        let mut rdr = self.config.build_reader(file, false);
        let mut rec = ByteRecord::new();
        skip_to_data(
            &mut rdr,
            self.config.skip_lines,
            self.config.has_headers,
            &mut rec,
        )?;
        let mut row_idx = 0usize;
        let total = self.row_offsets.len();

        while rdr
            .read_byte_record(&mut rec)
            .context("read CSV record while filtering")?
        {
            let cell = rec
                .get(col)
                .map(|b| self.config.decode_field(b))
                .unwrap_or_default();
            let hit = match mode {
                FilterMode::Contains => cell.contains(keyword),
                FilterMode::Equals => cell == keyword,
                FilterMode::UniqueByValue => seen.insert(cell),
            };
            if hit {
                out.push(row_idx);
            }
            row_idx += 1;
            if row_idx.is_multiple_of(10_000) && on_progress(row_idx, total) {
                return Ok(out);
            }
        }
        let _ = on_progress(row_idx, total);

        Ok(out)
    }
}

fn skip_to_data(
    rdr: &mut csv::Reader<File>,
    skip_lines: usize,
    has_headers: bool,
    rec: &mut ByteRecord,
) -> Result<()> {
    for _ in 0..skip_lines {
        if !rdr
            .read_byte_record(rec)
            .context("skip configured CSV record while querying")?
        {
            return Ok(());
        }
    }

    if has_headers {
        let _ = rdr
            .read_byte_record(rec)
            .context("skip CSV header while querying")?;
    }
    Ok(())
}
