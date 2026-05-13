use crate::core::CsvIndex;

#[derive(Clone, Debug)]
pub struct CsvSnapshot {
    pub headers: Vec<String>,
    pub row_count: usize,
}

impl From<&CsvIndex> for CsvSnapshot {
    fn from(index: &CsvIndex) -> Self {
        Self {
            headers: index.headers.clone(),
            row_count: index.row_offsets.len(),
        }
    }
}
