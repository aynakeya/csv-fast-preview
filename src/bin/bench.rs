use anyhow::{Context, Result};
use csvfastview::core::{CsvConfig, CsvEncoding, CsvIndex, FilterMode};
use std::env;
use std::time::Instant;

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

    println!("file={}", path);
    println!("rows={}, cols={}", idx.row_offsets.len(), idx.headers.len());
    println!("preview_first_ms={}", t_preview_first.as_millis());
    println!("index_ms={}", t_index.as_millis());
    println!("filter_contains_ms={}", t_filter.as_millis());
    println!("filter_hits={}", filtered.len());
    println!("preview_rows_loaded={}", preview.len());

    Ok(())
}
