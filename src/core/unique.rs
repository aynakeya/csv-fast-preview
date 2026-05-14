use anyhow::{Context, Result};
use csv::ByteRecord;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;

use super::index::CsvIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueValue {
    pub value: String,
    pub count: usize,
}

impl CsvIndex {
    pub fn unique_values_with_progress<F>(
        &self,
        col: usize,
        mut on_progress: F,
    ) -> Result<Vec<UniqueValue>>
    where
        F: FnMut(usize, usize) -> bool,
    {
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

        let total = self.row_offsets.len();
        let mut row_idx = 0usize;
        let mut counts = BTreeMap::<String, usize>::new();

        while rdr
            .read_byte_record(&mut rec)
            .context("read CSV record while indexing unique values")?
        {
            let value = rec
                .get(col)
                .map(|b| self.config.decode_field(b))
                .unwrap_or_default();
            *counts.entry(value).or_insert(0) += 1;
            row_idx += 1;
            if row_idx.is_multiple_of(10_000) && on_progress(row_idx, total) {
                break;
            }
        }
        let _ = on_progress(row_idx, total);

        Ok(counts
            .into_iter()
            .map(|(value, count)| UniqueValue { value, count })
            .collect())
    }

    pub fn filter_by_unique_values_with_progress<F>(
        &self,
        filters: &HashMap<usize, HashSet<String>>,
        mut on_progress: F,
    ) -> Result<Vec<usize>>
    where
        F: FnMut(usize, usize) -> bool,
    {
        let active: Vec<(&usize, &HashSet<String>)> = filters
            .iter()
            .filter(|(_, selected)| !selected.is_empty())
            .collect();
        if active.is_empty() {
            return Ok(self.all_rows());
        }

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

        let total = self.row_offsets.len();
        let mut row_idx = 0usize;
        let mut rows = Vec::new();

        while rdr
            .read_byte_record(&mut rec)
            .context("read CSV record while applying unique filters")?
        {
            let hit = active.iter().all(|(col, selected)| {
                let value = rec
                    .get(**col)
                    .map(|b| self.config.decode_field(b))
                    .unwrap_or_default();
                selected.contains(&value)
            });
            if hit {
                rows.push(row_idx);
            }
            row_idx += 1;
            if row_idx.is_multiple_of(10_000) && on_progress(row_idx, total) {
                break;
            }
        }
        let _ = on_progress(row_idx, total);
        Ok(rows)
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
