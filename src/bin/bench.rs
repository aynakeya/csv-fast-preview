use anyhow::{Context, Result};
use csvfastview::core::{CsvConfig, CsvEncoding, CsvIndex, FilterMode};
use std::env;
use std::fs::File;
use std::time::Instant;

const VISIBLE_READ_COLS: usize = 20;
const VISIBLE_READ_ROWS: usize = 64;
const VISIBLE_READ_WINDOWS: usize = 24;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .context("usage: cargo run --bin bench -- <csv_path> [delimiter] [encoding:utf8|gbk|gb18030|big5|shift-jis|iso-8859-1] [filter_col] [keyword]")?;

    let delimiter = args.next().and_then(|s| s.bytes().next()).unwrap_or(b',');
    let encoding = match args.next().as_deref() {
        Some("gbk") => CsvEncoding::Gbk,
        Some("gb18030") => CsvEncoding::Gb18030,
        Some("big5") => CsvEncoding::Big5,
        Some("shift-jis") => CsvEncoding::ShiftJis,
        Some("iso-8859-1") => CsvEncoding::Iso8859_1,
        _ => CsvEncoding::Utf8,
    };
    let filter_col = args
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let keyword = args.next().unwrap_or_default();

    let cfg = CsvConfig {
        delimiter,
        encoding,
        ..CsvConfig::default()
    };

    let t_preview0 = Instant::now();
    let preview_idx = CsvIndex::preview(&path, cfg.clone(), 200)?;
    let preview = preview_idx.read_page(&preview_idx.all_rows(), 0, 200)?;
    let t_preview_first = t_preview0.elapsed();

    let t0 = Instant::now();
    let idx = CsvIndex::build(&path, cfg)?;
    let t_index = t0.elapsed();

    let t2 = Instant::now();
    let filtered = idx.filter_rows(filter_col, &keyword, FilterMode::Contains)?;
    let t_filter = t2.elapsed();

    let t_unique0 = Instant::now();
    let unique = idx.index_unique_values_with_progress(filter_col, |_, _| false)?;
    let t_unique = t_unique0.elapsed();

    let visible_columns: Vec<usize> = (0..idx.headers.len().min(VISIBLE_READ_COLS)).collect();
    let t3 = Instant::now();
    let visible_rows_loaded = read_visible_windows(&idx, &visible_columns)?;
    let t_visible_read = t3.elapsed();

    println!("file={}", path);
    println!("rows={}, cols={}", idx.row_offsets.len(), idx.headers.len());
    println!("preview_first_ms={}", t_preview_first.as_millis());
    println!("index_ms={}", t_index.as_millis());
    println!("filter_contains_ms={}", t_filter.as_millis());
    println!("unique_index_ms={}", t_unique.as_millis());
    println!("visible_read_ms={}", t_visible_read.as_millis());
    println!("filter_hits={}", filtered.len());
    println!("unique_values={}", unique.values.len());
    println!("preview_rows_loaded={}", preview.len());
    println!("visible_rows_loaded={visible_rows_loaded}");

    Ok(())
}

fn read_visible_windows(idx: &CsvIndex, columns: &[usize]) -> Result<usize> {
    if idx.row_offsets.is_empty() || columns.is_empty() {
        return Ok(0);
    }

    let max_start = idx.row_offsets.len().saturating_sub(1);
    let step = (idx.row_offsets.len() / VISIBLE_READ_WINDOWS.max(1)).max(1);
    let mut rows_loaded = 0usize;
    let mut file = File::open(&idx.path)?;

    for window in 0..VISIBLE_READ_WINDOWS {
        let start = (window * step).min(max_start);
        let end = (start + VISIBLE_READ_ROWS).min(idx.row_offsets.len());
        let real_rows: Vec<usize> = (start..end).collect();
        let plan = idx.read_plan(&real_rows);
        rows_loaded += plan
            .read_columns_range_with_file(&mut file, columns, 0, real_rows.len())?
            .len();
    }

    Ok(rows_loaded)
}
