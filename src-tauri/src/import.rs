//! CSV / XLSX importer with column mapping for `phone` and `name`.

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize)]
pub struct ColumnMap {
    /// Column header (case-insensitive) for the phone field, e.g. "Phone Number".
    pub phone: String,
    /// Column header (case-insensitive) for the name field, e.g. "Name".
    pub name: String,
}

impl Default for ColumnMap {
    fn default() -> Self {
        Self {
            phone: "phone".into(),
            name: "name".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportedRow {
    pub phone: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportSummary {
    pub total_rows: usize,
    pub imported: usize,
    pub skipped: usize,
}

/// Import contacts from a CSV or XLSX file at `path` using the given column map.
/// File format is auto-detected from the extension.
pub fn import_file(path: &Path, map: &ColumnMap) -> AppResult<Vec<ImportedRow>> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" | "tsv" | "txt" => import_csv(path, map),
        "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" => import_xlsx(path, map),
        other => Err(AppError::invalid(format!(
            "unsupported file extension: {other}"
        ))),
    }
}

fn import_csv(path: &Path, map: &ColumnMap) -> AppResult<Vec<ImportedRow>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(path)?;

    let headers = rdr.headers()?.clone();
    let phone_idx = find_header(&headers, &map.phone)
        .ok_or_else(|| AppError::invalid(format!("column '{}' not found", map.phone)))?;
    let name_idx = find_header(&headers, &map.name);

    let mut out = Vec::new();
    for record in rdr.records() {
        let record = record?;
        let phone = record.get(phone_idx).unwrap_or("").trim().to_string();
        if phone.is_empty() {
            continue;
        }
        let name = name_idx
            .and_then(|i| record.get(i))
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(ImportedRow { phone, name });
    }
    Ok(out)
}

fn import_xlsx(path: &Path, map: &ColumnMap) -> AppResult<Vec<ImportedRow>> {
    let mut workbook = open_workbook_auto(path)?;
    let sheet_names = workbook.sheet_names();
    let first = sheet_names
        .first()
        .ok_or_else(|| AppError::invalid("workbook has no sheets"))?
        .clone();
    let range = workbook.worksheet_range(&first)?;

    let mut rows = range.rows();
    let header_row = rows
        .next()
        .ok_or_else(|| AppError::invalid("worksheet is empty"))?;
    let headers: Vec<String> = header_row.iter().map(cell_to_string).collect();
    let phone_idx = find_str(&headers, &map.phone)
        .ok_or_else(|| AppError::invalid(format!("column '{}' not found", map.phone)))?;
    let name_idx = find_str(&headers, &map.name);

    let mut out = Vec::new();
    for row in rows {
        let phone = row
            .get(phone_idx)
            .map(cell_to_string)
            .unwrap_or_default()
            .trim()
            .to_string();
        if phone.is_empty() {
            continue;
        }
        let name = name_idx
            .and_then(|i| row.get(i))
            .map(cell_to_string)
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push(ImportedRow { phone, name });
    }
    Ok(out)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            if f.fract() == 0.0 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{e:?}"),
    }
}

fn find_header(headers: &csv::StringRecord, target: &str) -> Option<usize> {
    let target = target.trim().to_ascii_lowercase();
    headers
        .iter()
        .position(|h| h.trim().to_ascii_lowercase() == target)
}

fn find_str(headers: &[String], target: &str) -> Option<usize> {
    let target = target.trim().to_ascii_lowercase();
    headers
        .iter()
        .position(|h| h.trim().to_ascii_lowercase() == target)
}
