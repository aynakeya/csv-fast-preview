use csv::ReaderBuilder;
use encoding_rs::{BIG5, GB18030, GBK, SHIFT_JIS};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CsvEncoding {
    Utf8,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    Iso8859_1,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CsvConfig {
    pub delimiter: u8,
    pub has_headers: bool,
    pub quote: u8,
    pub flexible: bool,
    pub encoding: CsvEncoding,
    pub skip_lines: usize,
}

impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            delimiter: b',',
            has_headers: true,
            quote: b'"',
            flexible: true,
            encoding: CsvEncoding::Utf8,
            skip_lines: 0,
        }
    }
}

impl CsvConfig {
    pub(crate) fn build_reader<R: Read>(&self, reader: R, has_headers: bool) -> csv::Reader<R> {
        ReaderBuilder::new()
            .delimiter(self.delimiter)
            .has_headers(has_headers)
            .quote(self.quote)
            .flexible(self.flexible)
            .from_reader(reader)
    }

    pub(crate) fn decode_field(&self, field: &[u8]) -> String {
        match self.encoding {
            CsvEncoding::Utf8 => String::from_utf8_lossy(field).into_owned(),
            CsvEncoding::Gbk => {
                let (cow, _, _) = GBK.decode(field);
                cow.into_owned()
            }
            CsvEncoding::Gb18030 => {
                let (cow, _, _) = GB18030.decode(field);
                cow.into_owned()
            }
            CsvEncoding::Big5 => {
                let (cow, _, _) = BIG5.decode(field);
                cow.into_owned()
            }
            CsvEncoding::ShiftJis => {
                let (cow, _, _) = SHIFT_JIS.decode(field);
                cow.into_owned()
            }
            CsvEncoding::Iso8859_1 => encoding_rs::mem::decode_latin1(field).into_owned(),
        }
    }
}
