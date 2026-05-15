use anyhow::{Context, Result};
use csv::ByteRecord;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use super::config::CsvConfig;
use super::index::CsvIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueValue {
    pub value: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct UniqueColumnIndex {
    pub values: Arc<[UniqueValue]>,
    row_groups: Vec<UniqueRowGroup>,
    rows: Vec<u32>,
}

#[derive(Clone, Debug)]
struct UniqueRowGroup {
    start: u32,
    len: u32,
}

impl UniqueColumnIndex {
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn rows_for_value_index(&self, value_idx: usize) -> impl Iterator<Item = usize> + '_ {
        let (start, end) = self
            .row_groups
            .get(value_idx)
            .map(|group| (group.start, group.start + group.len))
            .unwrap_or((0, 0));
        self.rows[start as usize..end as usize]
            .iter()
            .map(|row| *row as usize)
    }
}

impl CsvIndex {
    pub fn index_unique_values_with_progress<F>(
        &self,
        col: usize,
        on_progress: F,
    ) -> Result<UniqueColumnIndex>
    where
        F: FnMut(usize, usize) -> bool,
    {
        Self::index_unique_values_for_file(
            &self.path,
            &self.config,
            self.row_offsets.len(),
            col,
            on_progress,
        )
    }

    pub fn index_unique_values_for_file<F>(
        path: &Path,
        config: &CsvConfig,
        total_rows: usize,
        col: usize,
        mut on_progress: F,
    ) -> Result<UniqueColumnIndex>
    where
        F: FnMut(usize, usize) -> bool,
    {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut rdr = config.build_reader(file, false);
        let mut rec = ByteRecord::new();
        skip_to_data(&mut rdr, config.skip_lines, config.has_headers, &mut rec)?;

        let mut row_idx = 0usize;
        let mut ids = HashMap::<String, usize>::new();
        let mut counts = Vec::<u32>::new();
        let mut row_value_ids = Vec::<u32>::with_capacity(total_rows);

        while rdr
            .read_byte_record(&mut rec)
            .context("read CSV record while indexing unique values")?
        {
            let value = rec
                .get(col)
                .map(|b| config.decode_field(b))
                .unwrap_or_default();
            let id = if let Some(id) = ids.get(&value).copied() {
                id
            } else {
                let id = counts.len();
                ids.insert(value, id);
                counts.push(0);
                id
            };
            counts[id] = counts[id]
                .checked_add(1)
                .context("too many rows for one unique value")?;
            row_value_ids.push(u32::try_from(id).context("too many unique values to index")?);
            row_idx += 1;
            if row_idx.is_multiple_of(10_000) && on_progress(row_idx, total_rows) {
                break;
            }
        }
        let _ = on_progress(row_idx, total_rows);

        let mut values_by_id: Vec<(usize, String)> =
            ids.into_iter().map(|(value, id)| (id, value)).collect();
        values_by_id.sort_by(|(_, left), (_, right)| left.cmp(right));

        let mut sorted_values = Vec::with_capacity(values_by_id.len());
        let mut row_groups = Vec::with_capacity(values_by_id.len());
        let mut sorted_rows = vec![0u32; row_value_ids.len()];
        let mut old_id_to_cursor = vec![0u32; counts.len()];
        let mut start = 0u32;
        for (old_id, value) in values_by_id {
            let len = counts[old_id];
            old_id_to_cursor[old_id] = start;
            sorted_values.push(UniqueValue {
                value,
                count: len as usize,
            });
            row_groups.push(UniqueRowGroup { start, len });
            start = start.checked_add(len).context("too many rows to index")?;
        }

        for (row_idx, old_id) in row_value_ids.into_iter().enumerate() {
            let cursor = &mut old_id_to_cursor[old_id as usize];
            sorted_rows[*cursor as usize] =
                u32::try_from(row_idx).context("too many rows to index unique values")?;
            *cursor += 1;
        }

        Ok(UniqueColumnIndex {
            values: Arc::from(sorted_values),
            row_groups,
            rows: sorted_rows,
        })
    }

    pub fn unique_values_with_progress<F>(
        &self,
        col: usize,
        mut on_progress: F,
    ) -> Result<Arc<[UniqueValue]>>
    where
        F: FnMut(usize, usize) -> bool,
    {
        Ok(self
            .index_unique_values_with_progress(col, &mut on_progress)?
            .values)
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
