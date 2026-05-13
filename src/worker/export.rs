use crate::core::CsvIndex;

pub(super) fn export_rows(
    index: &CsvIndex,
    path: &str,
    row_indices: &[usize],
    visible_columns: &[usize],
) -> anyhow::Result<()> {
    let mut wtr = csv::Writer::from_path(path)?;
    let headers: Vec<String> = visible_columns
        .iter()
        .map(|i| index.headers.get(*i).cloned().unwrap_or_default())
        .collect();
    wtr.write_record(headers)?;
    for row_idx in row_indices {
        if let Ok(mut rows) = index.read_page(&[*row_idx], 0, 1) {
            if let Some(row) = rows.pop() {
                let rec: Vec<String> = visible_columns
                    .iter()
                    .map(|i| row.get(*i).cloned().unwrap_or_default())
                    .collect();
                wtr.write_record(rec)?;
            }
        }
    }
    wtr.flush()?;
    Ok(())
}
