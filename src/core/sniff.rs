use anyhow::{Context, Result};
use encoding_rs::{BIG5, GB18030, GBK, SHIFT_JIS};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::config::CsvEncoding;

#[derive(Clone, Debug)]
pub struct CsvSniffResult {
    pub delimiter: u8,
    pub has_headers: bool,
    pub encoding: CsvEncoding,
}

pub fn sniff_csv(path: impl AsRef<Path>) -> Result<CsvSniffResult> {
    sniff_csv_with_skip(path, 0)
}

pub fn sniff_csv_with_skip(path: impl AsRef<Path>, skip_lines: usize) -> Result<CsvSniffResult> {
    let path = path.as_ref();
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    let encoding = detect_encoding(&buf);
    let sample = String::from_utf8_lossy(&buf);
    let mut lines = sample
        .lines()
        .skip(skip_lines)
        .filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or_default();
    let second = lines.next().unwrap_or_default();

    let candidates = [b',', b'\t', b';', b'|'];
    let mut best = b',';
    let mut best_score = 0usize;
    for d in candidates {
        let c1 = first.as_bytes().iter().filter(|b| **b == d).count();
        let c2 = second.as_bytes().iter().filter(|b| **b == d).count();
        let score = c1.min(c2).max(c1);
        if score > best_score {
            best_score = score;
            best = d;
        }
    }

    let f1: Vec<&str> = first.split(best as char).collect();
    let f2: Vec<&str> = second.split(best as char).collect();
    let has_headers = if f1.is_empty() || f2.is_empty() {
        true
    } else {
        let first_looks_text = f1.iter().any(|v| v.chars().any(|c| c.is_alphabetic()));
        let second_looks_data =
            f2.iter().any(|v| v.parse::<f64>().is_ok()) || f2.iter().any(|v| !v.is_empty());
        first_looks_text && second_looks_data
    };

    Ok(CsvSniffResult {
        delimiter: best,
        has_headers,
        encoding,
    })
}

fn detect_encoding(bytes: &[u8]) -> CsvEncoding {
    if std::str::from_utf8(bytes).is_ok() {
        return CsvEncoding::Utf8;
    }
    let candidates = [
        (
            CsvEncoding::Gb18030,
            decode_preview(CsvEncoding::Gb18030, bytes),
        ),
        (CsvEncoding::Gbk, decode_preview(CsvEncoding::Gbk, bytes)),
        (CsvEncoding::Big5, decode_preview(CsvEncoding::Big5, bytes)),
        (
            CsvEncoding::ShiftJis,
            decode_preview(CsvEncoding::ShiftJis, bytes),
        ),
        (
            CsvEncoding::Iso8859_1,
            decode_preview(CsvEncoding::Iso8859_1, bytes),
        ),
    ];
    candidates
        .into_iter()
        .max_by_key(|(_, score)| *score)
        .map(|(enc, _)| enc)
        .unwrap_or(CsvEncoding::Gb18030)
}

fn decode_preview(enc: CsvEncoding, bytes: &[u8]) -> isize {
    let txt = match enc {
        CsvEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        CsvEncoding::Gbk => GBK.decode(bytes).0.into_owned(),
        CsvEncoding::Gb18030 => GB18030.decode(bytes).0.into_owned(),
        CsvEncoding::Big5 => BIG5.decode(bytes).0.into_owned(),
        CsvEncoding::ShiftJis => SHIFT_JIS.decode(bytes).0.into_owned(),
        CsvEncoding::Iso8859_1 => encoding_rs::mem::decode_latin1(bytes).into_owned(),
    };
    let mut score = 0isize;
    for ch in txt.chars().take(4096) {
        if ch == '\u{FFFD}' {
            score -= 50;
        } else if ch.is_ascii_graphic() || ch.is_ascii_whitespace() {
            score += 1;
        } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            score += 3;
        } else if ch.is_control() {
            score -= 2;
        }
    }
    score
}
