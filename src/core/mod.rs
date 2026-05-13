mod config;
mod index;
mod query;
mod sniff;

pub use config::{CsvConfig, CsvEncoding};
pub use index::CsvIndex;
pub use query::FilterMode;
#[allow(unused_imports)]
pub use sniff::{CsvSniffResult, sniff_csv, sniff_csv_with_skip};

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::GBK;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_temp_bytes(content: &[u8]) -> PathBuf {
        for attempt in 0..32u32 {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("csvfastview-test-{id}-{attempt}.csv"));
            let file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path);
            match file {
                Ok(mut f) => {
                    f.write_all(content).expect("write temp csv");
                    return path;
                }
                Err(_) => continue,
            }
        }
        panic!("cannot allocate temp csv path")
    }

    #[test]
    fn builds_index_and_reads_page() {
        let path = write_temp_bytes(b"name,age\nanna,18\nbob,20\n");
        let cfg = CsvConfig::default();
        let idx = CsvIndex::build(&path, cfg).expect("build index");
        assert_eq!(idx.headers, vec!["name", "age"]);
        assert_eq!(idx.row_offsets.len(), 2);

        let rows = idx.read_page(&idx.all_rows(), 0, 2).expect("read page");
        assert_eq!(rows[0][0], "anna");
        assert_eq!(rows[1][1], "20");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn supports_custom_delimiter_and_filters() {
        let path = write_temp_bytes(b"city|cat\nshanghai|a\nshenzhen|b\nshanghai|c\n");
        let cfg = CsvConfig {
            delimiter: b'|',
            ..CsvConfig::default()
        };
        let idx = CsvIndex::build(&path, cfg).expect("build index");

        let contains = idx
            .filter_rows(0, "shang", FilterMode::Contains)
            .expect("contains");
        assert_eq!(contains.len(), 2);

        let equals = idx
            .filter_rows(0, "shenzhen", FilterMode::Equals)
            .expect("equals");
        assert_eq!(equals.len(), 1);

        let unique = idx
            .filter_rows(0, "", FilterMode::UniqueByValue)
            .expect("unique by value");
        assert_eq!(unique.len(), 2);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn supports_gbk_decoding() {
        let txt = "城市,值\n上海,1\n深圳,2\n";
        let (bytes, _, _) = GBK.encode(txt);
        let path = write_temp_bytes(bytes.as_ref());
        let cfg = CsvConfig {
            encoding: CsvEncoding::Gbk,
            ..CsvConfig::default()
        };
        let idx = CsvIndex::build(&path, cfg).expect("build index");
        assert_eq!(idx.headers[0], "城市");

        let rows = idx.read_page(&idx.all_rows(), 0, 2).expect("read page");
        assert_eq!(rows[0][0], "上海");

        let hit = idx
            .filter_rows(0, "深圳", FilterMode::Contains)
            .expect("filter");
        assert_eq!(hit.len(), 1);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn sniffs_delimiter_and_headers() {
        let path = write_temp_bytes(b"name;age;city\nalice;18;sh\nbob;20;sz\n");
        let sniff = sniff_csv(&path).expect("sniff");
        assert_eq!(sniff.delimiter, b';');
        assert!(sniff.has_headers);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn skips_records_before_headers_and_data() {
        let path = write_temp_bytes(b"comment one\ncomment two\nname,age\nanna,18\nbob,20\n");
        let cfg = CsvConfig {
            skip_lines: 2,
            ..CsvConfig::default()
        };
        let idx = CsvIndex::build(&path, cfg).expect("build index");
        assert_eq!(idx.headers, vec!["name", "age"]);
        assert_eq!(idx.row_offsets.len(), 2);

        let rows = idx.read_page(&idx.all_rows(), 0, 2).expect("read page");
        assert_eq!(rows[0], vec!["anna", "18"]);
        assert_eq!(rows[1], vec!["bob", "20"]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn sniffs_after_skipped_lines() {
        let path = write_temp_bytes(b"metadata\nname|age|city\nalice|18|sh\nbob|20|sz\n");
        let sniff = sniff_csv_with_skip(&path, 1).expect("sniff");
        assert_eq!(sniff.delimiter, b'|');
        assert!(sniff.has_headers);
        let _ = fs::remove_file(path);
    }
}
