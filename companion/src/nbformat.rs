//! nbformat parsing, validation, and document-level edit operations.

use serde::Serialize;
use tower_lsp::lsp_types;
use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value};

pub const SOURCE: &str = "ipynb-lsp";

/// Parses notebook JSON. The default serde_json `Value` uses `BTreeMap` for
/// objects, so re-serializing yields sorted keys — the same canonical order
/// nbformat writes (`sort_keys=True`).
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text)
}

/// Canonical serialization matching nbformat's writer style: 1-space indent,
/// sorted keys, trailing newline.
pub fn serialize_pretty(value: &Value) -> String {
    let mut buf = Vec::new();
    let formatter = PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    // Serialization of a parsed JSON value cannot fail.
    let _ = value.serialize(&mut serializer);
    buf.push(b'\n');
    String::from_utf8(buf).unwrap_or_else(|_| value.to_string())
}

/// Structural validation against nbformat v4.
pub fn validate(text: &str) -> Vec<Diagnostic> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(err) => {
            let position = Position::new(
                (err.line() as u32).saturating_sub(1),
                (err.column() as u32).saturating_sub(1),
            );
            return vec![error(
                Range::new(position, position),
                format!("Invalid JSON: {err}"),
                "invalid-json",
            )];
        }
    };

    let mut diagnostics = Vec::new();
    let cell_type_lines = scan_cell_type_lines(text);

    let root = match value.as_object() {
        Some(object) => object,
        None => {
            diagnostics.push(error(
                Range::new(Position::new(0, 0), Position::new(0, 0)),
                "Notebook root must be a JSON object".to_string(),
                "nbformat-root",
            ));
            return diagnostics;
        }
    };

    let nbformat = root.get("nbformat");
    match nbformat {
        Some(Value::Number(number)) if number.as_i64() == Some(4) => {}
        Some(Value::Number(number)) => diagnostics.push(warning(
            doc_range(),
            format!("nbformat version {} is not fully supported; version 4 is expected", number),
            "nbformat-version",
        )),
        None => diagnostics.push(warning(
            doc_range(),
            "Missing \"nbformat\" field; notebooks should declare nbformat: 4".to_string(),
            "nbformat-version",
        )),
        _ => diagnostics.push(warning(
            doc_range(),
            "\"nbformat\" must be a number".to_string(),
            "nbformat-version",
        )),
    }

    match root.get("cells") {
        Some(Value::Array(cells)) => {
            let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            for (index, cell) in cells.iter().enumerate() {
                let range = cell_range(&cell_type_lines, index);
                validate_cell(cell, index, &mut diagnostics, range, &mut seen_ids);
            }
        }
        Some(_) => diagnostics.push(error(
            doc_range(),
            "\"cells\" must be an array of cell objects".to_string(),
            "nbformat-cells",
        )),
        None => diagnostics.push(error(
            doc_range(),
            "Missing \"cells\" array".to_string(),
            "nbformat-cells",
        )),
    }

    if let Some(metadata) = root.get("metadata") {
        if !metadata.is_object() {
            diagnostics.push(error(
                doc_range(),
                "\"metadata\" must be an object".to_string(),
                "nbformat-metadata",
            ));
        }
    }

    diagnostics
}

fn validate_cell(
    cell: &Value,
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
    range: Range,
    seen_ids: &mut std::collections::HashSet<String>,
) {
    let object = match cell.as_object() {
        Some(object) => object,
        None => {
            diagnostics.push(error(
                range,
                format!("Cell {index} is not an object"),
                "nbformat-cell",
            ));
            return;
        }
    };

    let cell_type = object.get("cell_type").and_then(Value::as_str);

    match cell_type {
        Some("code") | Some("markdown") | Some("raw") => {}
        Some(other) => {
            diagnostics.push(error(
                range,
                format!("Cell {index} has invalid cell_type \"{other}\"; expected \"code\", \"markdown\", or \"raw\""),
                "nbformat-cell-type",
            ));
        }
        None => {
            diagnostics.push(error(
                range,
                format!("Cell {index} is missing \"cell_type\""),
                "nbformat-cell-type",
            ));
        }
    }

    if let Some(id) = object.get("id") {
        match id.as_str() {
            Some(id) if !seen_ids.insert(id.to_string()) => {
                diagnostics.push(error(
                    range,
                    format!("Cell {index} has duplicate id \"{id}\""),
                    "nbformat-cell-id",
                ));
            }
            Some(_) => {}
            None => diagnostics.push(error(
                range,
                format!("Cell {index} \"id\" must be a string"),
                "nbformat-cell-id",
            )),
        }
    }

    match object.get("source") {
        Some(Value::String(_)) => {}
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                if item.as_str().is_none() {
                    diagnostics.push(error(
                        range,
                        format!("Cell {index} source[{i}] must be a string"),
                        "nbformat-source",
                    ));
                }
            }
        }
        Some(_) => diagnostics.push(error(
            range,
            format!("Cell {index} \"source\" must be a string or array of strings"),
            "nbformat-source",
        )),
        None => diagnostics.push(error(
            range,
            format!("Cell {index} is missing \"source\""),
            "nbformat-source",
        )),
    }

    if cell_type == Some("code") {
        if let Some(outputs) = object.get("outputs") {
            if !outputs.is_array() {
                diagnostics.push(error(
                    range,
                    format!("Cell {index} \"outputs\" must be an array"),
                    "nbformat-outputs",
                ));
            }
        } else {
            diagnostics.push(warning(
                range,
                format!("Code cell {index} is missing \"outputs\""),
                "nbformat-outputs",
            ));
        }

        if let Some(count) = object.get("execution_count") {
            if !count.is_number() && !count.is_null() {
                diagnostics.push(warning(
                    range,
                    format!("Cell {index} \"execution_count\" must be a number or null"),
                    "nbformat-execution-count",
                ));
            }
        }
    }

    if let Some(metadata) = object.get("metadata") {
        if !metadata.is_object() {
            diagnostics.push(error(
                range,
                format!("Cell {index} \"metadata\" must be an object"),
                "nbformat-cell-metadata",
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Edit operations (each returns a canonicalized whole-document value)
// ---------------------------------------------------------------------------

pub fn has_outputs(value: &Value) -> bool {
    cells(value).into_iter().flatten().any(|cell| {
        cell.get("cell_type").and_then(Value::as_str) == Some("code")
            && cell.get("outputs").and_then(Value::as_array).map(|o| !o.is_empty()).unwrap_or(false)
    })
}

pub fn empty_cell_count(value: &Value) -> usize {
    cells(value).into_iter().flatten().filter(|cell| cell_is_empty(cell)).count()
}

fn cell_is_empty(cell: &Map<String, Value>) -> bool {
    match cell.get("source") {
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Array(items)) => items.iter().all(|item| {
            item.as_str().map(str::trim).map(str::is_empty).unwrap_or(true)
        }),
        _ => true,
    }
}

/// Sets every code cell's outputs to `[]` and execution_count to null.
pub fn clear_outputs(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(Value::Array(cells)) = value.get_mut("cells") {
        for cell in cells.iter_mut() {
            if let Some(object) = cell.as_object_mut() {
                if object.get("cell_type").and_then(Value::as_str) == Some("code") {
                    if let Some(outputs) = object.get_mut("outputs") {
                        if outputs.is_array() {
                            *outputs = Value::Array(Vec::new());
                        }
                    }
                    if let Some(count) = object.get_mut("execution_count") {
                        if count.is_number() {
                            *count = Value::Null;
                        }
                    }
                }
            }
        }
    }
    value
}

/// Removes cells whose source is empty/whitespace-only.
pub fn strip_empty_cells(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(Value::Array(cells)) = value.get_mut("cells") {
        cells.retain(|cell| !cell.as_object().map(cell_is_empty).unwrap_or(true));
    }
    value
}

/// Appends a new empty cell of the given type.
pub fn add_cell(value: &Value, cell_type: &str) -> Value {
    let mut value = value.clone();
    let mut cell = Map::new();
    cell.insert("cell_type".to_string(), Value::String(cell_type.to_string()));
    if cell_type == "code" {
        cell.insert("execution_count".to_string(), Value::Null);
    }
    cell.insert("id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));
    cell.insert("metadata".to_string(), Value::Object(Map::new()));
    if cell_type == "code" {
        cell.insert("outputs".to_string(), Value::Array(Vec::new()));
    }
    cell.insert("source".to_string(), Value::String(String::new()));

    match value.get_mut("cells") {
        Some(Value::Array(cells)) => cells.push(Value::Object(cell)),
        _ => {
            value
                .as_object_mut()
                .map(|object| object.insert("cells".to_string(), Value::Array(vec![Value::Object(cell)])));
        }
    }
    value
}

// ---------------------------------------------------------------------------
// Position helpers
// ---------------------------------------------------------------------------

fn cells(value: &Value) -> Vec<Option<&Map<String, Value>>> {
    value
        .get("cells")
        .and_then(Value::as_array)
        .map(|c| c.iter().map(Value::as_object).collect())
        .unwrap_or_default()
}

/// Line numbers (0-based) of `"cell_type"` key occurrences in raw text.
/// nbformat always writes `cell_type` first for every cell, so the n-th
/// occurrence belongs to the n-th cell. Heuristic: a cell whose
/// `"cell_type"` line can't be found falls back to the document start.
fn scan_cell_type_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains("\"cell_type\""))
        .map(|(line, _)| line)
        .collect()
}

fn cell_range(cell_type_lines: &[usize], index: usize) -> Range {
    let line = cell_type_lines.get(index).copied().unwrap_or(0);
    Range::new(Position::new(line as u32, 0), Position::new(line as u32 + 1, 0))
}

fn doc_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

fn error(range: Range, message: String, code: &str) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(lsp_types::NumberOrString::String(code.to_string())),
        source: Some(SOURCE.to_string()),
        message,
        ..Diagnostic::default()
    }
}

fn warning(range: Range, message: String, code: &str) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(lsp_types::NumberOrString::String(code.to_string())),
        source: Some(SOURCE.to_string()),
        message,
        ..Diagnostic::default()
    }
}

