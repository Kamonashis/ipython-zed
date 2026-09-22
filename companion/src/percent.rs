//! Conversion between nbformat notebooks and the "percent" cell format
//! (`# %%`-separated python scripts, jupytext-light compatible for code and
//! markdown cells).
//!
//! Canonical script layout:
//!
//! ```text
//! # %%
//! import numpy
//!
//! # %% [markdown]
//! # # Heading
//! # body text
//!
//! # %% [raw]
//! # raw content
//! ```
//!
//! Limitations (inherited from the format itself):
//! - Cell metadata is not stored in the script; the notebook keeps it.
//! - A python comment line that is exactly `# %%` would be misinterpreted as
//!   a cell marker (same trade-off as jupytext).
//! - Notebook-level metadata lives only in the .ipynb side.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// `foo.ipynb` -> `foo.nb.py`
pub fn script_path_for(notebook_path: &Path) -> PathBuf {
    let stem = notebook_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("notebook");
    notebook_path.with_file_name(format!("{stem}.nb.py"))
}

/// Concatenated text of a cell's source (string or array-of-lines form).
pub fn cell_source_text(cell: &Map<String, Value>) -> String {
    match cell.get("source") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Renders a notebook into the percent script format.
pub fn notebook_to_percent(notebook: &Value) -> Result<String, String> {
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| "notebook has no cells array".to_string())?;

    let mut script = String::new();

    for (index, cell) in cells.iter().enumerate() {
        let object = cell
            .as_object()
            .ok_or_else(|| format!("cell {index} is not an object"))?;
        let cell_type = object
            .get("cell_type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("cell {index} has no cell_type"))?;

        if index > 0 {
            script.push('\n');
        }

        match cell_type {
            "code" => script.push_str("# %%\n"),
            "markdown" => script.push_str("# %% [markdown]\n"),
            "raw" => script.push_str("# %% [raw]\n"),
            other => return Err(format!("cell {index} has invalid cell_type \"{other}\"")),
        }

        let mut source = cell_source_text(object);
        if !source.is_empty() && !source.ends_with('\n') {
            source.push('\n');
        }
        match cell_type {
            "code" => script.push_str(&source),
            _ => {
                for line in source.lines() {
                    if line.trim().is_empty() {
                        script.push_str("#\n");
                    } else {
                        script.push_str("# ");
                        script.push_str(line);
                        script.push('\n');
                    }
                }
            }
        }
    }

    Ok(script)
}

/// Parses a percent script back into a notebook. `base_metadata` is reused as
/// the notebook metadata (pairing must never lose notebook-level metadata that
/// lives only on the .ipynb side).
pub fn percent_to_notebook(script: &str, base_metadata: Option<&Value>) -> Result<Value, String> {
    let mut cells: Vec<Value> = Vec::new();
    let mut current_type: Option<&'static str> = None;
    let mut current_metadata: Value = Value::Object(Map::new());
    let mut body: Vec<String> = Vec::new();

    macro_rules! flush {
        () => {
            if let Some(cell_type) = current_type.take() {
                // Trailing blank lines before the next marker are cell
                // separators, not content. Interior blank lines (rendered as
                // `#`) are kept.
                while body.last().map(String::is_empty).unwrap_or(false) {
                    body.pop();
                }
                cells.push(make_cell(cell_type, &body, &current_metadata));
                body.clear();
            }
        };
    }

    for line in script.lines() {
        if let Some(marker) = parse_marker(line) {
            flush!();
            match marker {
                Marker::Code { metadata } => {
                    current_type = Some("code");
                    current_metadata = metadata;
                }
                Marker::Markdown { metadata } => {
                    current_type = Some("markdown");
                    current_metadata = metadata;
                }
                Marker::Raw { metadata } => {
                    current_type = Some("raw");
                    current_metadata = metadata;
                }
            }
        } else if current_type.is_some() {
            let line = match current_type {
                Some("code") => line.to_string(),
                _ => strip_comment_prefix(line),
            };
            body.push(line);
        }
    }
    flush!();

    let metadata = base_metadata
        .and_then(Value::as_object)
        .map(|object| Value::Object(object.clone()))
        .unwrap_or_else(default_metadata);

    Ok(Value::Object({
        let mut object = Map::new();
        object.insert("cells".to_string(), Value::Array(cells));
        object.insert("metadata".to_string(), metadata);
        object.insert("nbformat".to_string(), Value::from(4));
        object.insert("nbformat_minor".to_string(), Value::from(5));
        object
    }))
}

enum Marker {
    Code { metadata: Value },
    Markdown { metadata: Value },
    Raw { metadata: Value },
}

fn parse_marker(line: &str) -> Option<Marker> {
    let rest = line.strip_prefix("# %%")?;

    let (tag, rest) = if let Some(rest) = rest.strip_prefix(" [markdown]") {
        ("markdown", rest)
    } else if let Some(rest) = rest.strip_prefix(" [raw]") {
        ("raw", rest)
    } else {
        ("code", rest)
    };

    let metadata = rest
        .trim()
        .strip_prefix('{')
        .and_then(|_| serde_json::from_str::<Value>(rest.trim()).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));

    Some(match tag {
        "markdown" => Marker::Markdown { metadata },
        "raw" => Marker::Raw { metadata },
        _ => Marker::Code { metadata },
    })
}

fn strip_comment_prefix(line: &str) -> String {
    if line == "#" {
        String::new()
    } else if let Some(rest) = line.strip_prefix("# ") {
        rest.to_string()
    } else {
        line.to_string()
    }
}

fn make_cell(cell_type: &str, body: &[String], metadata: &Value) -> Value {
    let mut object = Map::new();
    object.insert("cell_type".to_string(), Value::String(cell_type.to_string()));

    let joined = join_lines(body);
    if cell_type == "code" {
        object.insert("execution_count".to_string(), Value::Null);
        object.insert("outputs".to_string(), Value::Array(Vec::new()));
    }
    object.insert("id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));
    object.insert("metadata".to_string(), metadata.clone());
    object.insert(
        "source".to_string(),
        source_array(&joined),
    );
    Value::Object(object)
}

/// Splits source text into nbformat's canonical array-of-lines form
/// (every line ends with `\n`; the final line stays bare if the text does
/// not end with one).
pub fn source_array(text: &str) -> Value {
    if text.is_empty() {
        return Value::Array(Vec::new());
    }
    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if ch == '\n' {
            items.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    Value::Array(items.into_iter().map(Value::String).collect())
}

/// Lines joined with `\n` and a trailing newline. Notebooks are
/// canonicalized: a source ending without `\n` gains one on import, which
/// keeps script->notebook->script byte-stable.
fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn default_metadata() -> Value {
    Value::Object({
        let mut kernelspec = Map::new();
        kernelspec.insert("display_name".to_string(), Value::from("Python 3 (ipykernel)"));
        kernelspec.insert("language".to_string(), Value::from("python"));
        kernelspec.insert("name".to_string(), Value::from("python3"));

        let mut metadata = Map::new();
        metadata.insert("kernelspec".to_string(), Value::Object(kernelspec));
        metadata
    })
}

/// Merges a script-imported notebook back into the current notebook: the
/// script's cell content wins, but outputs, execution counts and per-cell
/// metadata are preserved from the current notebook by code-cell ordinal.
pub fn merge_import_with_notebook(imported: &Value, current: &Value) -> Value {
    let mut merged = imported.clone();

    // Keep the original nbformat version fields and notebook-level metadata:
    // the script is the content side, the notebook is the metadata side.
    if let (Some(merged_object), Some(current_object)) =
        (merged.as_object_mut(), current.as_object())
    {
        for key in ["nbformat", "nbformat_minor", "metadata"] {
            if let Some(value) = current_object.get(key) {
                merged_object.insert(key.to_string(), value.clone());
            }
        }
    }

    let Some(Some(cells)) = merged.get_mut("cells").map(Value::as_array_mut) else {
        return merged;
    };
    let current_cells = current
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut current_code_index = 0;
    let current_code_cells: Vec<&Map<String, Value>> = current_cells
        .iter()
        .filter_map(Value::as_object)
        .filter(|cell| cell.get("cell_type").and_then(Value::as_str) == Some("code"))
        .collect();

    for cell in cells.iter_mut() {
        let Some(object) = cell.as_object_mut() else {
            continue;
        };
        if object.get("cell_type").and_then(Value::as_str) != Some("code") {
            continue;
        }
        if let Some(previous) = current_code_cells.get(current_code_index) {
            if let Some(outputs) = previous.get("outputs").filter(|o| o.is_array()) {
                object.insert("outputs".to_string(), outputs.clone());
            }
            if let Some(count) = previous.get("execution_count") {
                if count.is_number() || count.is_null() {
                    object.insert("execution_count".to_string(), count.clone());
                }
            }
            if let Some(metadata) = previous.get("metadata").filter(|m| m.is_object()) {
                object.insert("metadata".to_string(), metadata.clone());
            }
        }
        current_code_index += 1;
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbformat;

    fn round_trip(notebook: &str) {
        let nb = nbformat::parse(notebook).expect("parse notebook");
        let script = notebook_to_percent(&nb).expect("to percent");
        let imported = percent_to_notebook(&script, nb.get("metadata")).expect("to notebook");
        let script2 = notebook_to_percent(&imported).expect("to percent 2");
        assert_eq!(script, script2, "script must be stable across round trip");
    }

    #[test]
    fn code_and_markdown_round_trip() {
        round_trip(include_str!("../../fixtures/sample.ipynb"));
    }

    #[test]
    fn script_layout() {
        let nb = nbformat::parse(
            r##"{"cells": [{"cell_type": "code", "id": "a", "metadata": {}, "outputs": [], "source": ["import x\n", "x()"]}, {"cell_type": "markdown", "id": "b", "metadata": {}, "source": ["# Title\n", "body"]}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"##,
        )
        .unwrap();
        let script = notebook_to_percent(&nb).unwrap();
        assert_eq!(
            script,
            "# %%\nimport x\nx()\n\n# %% [markdown]\n# # Title\n# body\n"
        );

        let imported = percent_to_notebook(&script, None).unwrap();
        let script2 = notebook_to_percent(&imported).unwrap();
        assert_eq!(script, script2);
    }

    #[test]
    fn empty_markdown_lines() {
        let nb = nbformat::parse(
            r##"{"cells": [{"cell_type": "markdown", "id": "b", "metadata": {}, "source": ["para\n", "\n", "para2"]}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"##,
        )
        .unwrap();
        let script = notebook_to_percent(&nb).unwrap();
        assert_eq!(script, "# %% [markdown]\n# para\n#\n# para2\n");
        let imported = percent_to_notebook(&script, None).unwrap();
        assert_eq!(notebook_to_percent(&imported).unwrap(), script);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::nbformat;

    #[test]
    fn merge_preserves_outputs_by_ordinal() {
        let imported = nbformat::parse(
            r##"{"cells": [{"cell_type": "code", "execution_count": 7, "id": "n", "metadata": {}, "outputs": [], "source": "a = 1"}, {"cell_type": "markdown", "id": "m", "metadata": {}, "source": "text"}, {"cell_type": "code", "execution_count": null, "id": "o", "metadata": {}, "outputs": [], "source": "b = 2"}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"##,
        )
        .unwrap();
        let current = nbformat::parse(
            r##"{"cells": [{"cell_type": "code", "execution_count": 3, "id": "old1", "metadata": {"tags": ["x"]}, "outputs": [{"output_type": "stream", "name": "stdout", "text": ["out\n"]}], "source": ["a = 1\n"]}, {"cell_type": "markdown", "id": "old2", "metadata": {}, "source": ["text"]}, {"cell_type": "code", "execution_count": 9, "id": "old3", "metadata": {}, "outputs": [], "source": ["b = 2\n"]}], "metadata": {"kernelspec": {"name": "python3"}}, "nbformat": 4, "nbformat_minor": 5}"##,
        )
        .unwrap();

        let merged = merge_import_with_notebook(&imported, &current);
        let cells = merged.get("cells").unwrap().as_array().unwrap();

        // first code cell keeps outputs + count + metadata from current
        assert_eq!(cells[0].get("outputs").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(cells[0].get("execution_count").unwrap().as_i64(), Some(3));
        assert_eq!(cells[0].get("metadata").unwrap().get("tags").is_some(), true);
        assert_eq!(cells[0].get("source").unwrap().as_str(), Some("a = 1"));

        // second code cell has no outputs in current (empty), keeps count
        assert_eq!(cells[2].get("execution_count").unwrap().as_i64(), Some(9));

        // notebook metadata survives
        assert!(merged.get("metadata").unwrap().get("kernelspec").is_some());
    }

    #[test]
    fn canonical_serialization_is_sorted_and_stable() {
        let text = r##"{"nbformat": 4, "metadata": {}, "cells": [{"cell_type": "code", "outputs": [], "source": ["x=1"], "id": "a", "metadata": {}}]}"##;
        let nb = nbformat::parse(text).unwrap();
        let pretty = nbformat::serialize_pretty(&nb);
        let reparsed = nbformat::parse(&pretty).unwrap();
        assert_eq!(nbformat::serialize_pretty(&reparsed), pretty);
        assert!(pretty.starts_with("{\n \"cells\""));
    }
}

#[cfg(test)]
mod validate_tests {
    use crate::nbformat;

    #[test]
    fn invalid_json_reports_position() {
        let diags = nbformat::validate("{\n \"cells\": [}\n}");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.as_ref().map(|c| match c {
            tower_lsp::lsp_types::NumberOrString::String(s) => s.clone(),
            _ => String::new(),
        }), Some("invalid-json".to_string()));
        assert!(diags[0].range.start.line > 0);
    }

    #[test]
    fn empty_text_is_clean() {
        assert!(nbformat::validate("").is_empty());
        assert!(nbformat::validate("   \n  ").is_empty());
    }

    #[test]
    fn duplicate_ids_and_bad_cell_type() {
        let text = r##"{
 "cells": [
  {"cell_type": "code", "id": "dup", "metadata": {}, "outputs": [], "source": ["x"]},
  {"cell_type": "quiz", "id": "dup", "metadata": {}, "source": []}
 ],
 "metadata": {},
 "nbformat": 4
}"##;
        let diags = nbformat::validate(text);
        let codes: Vec<&str> = diags.iter().filter_map(|d| match &d.code {
            Some(tower_lsp::lsp_types::NumberOrString::String(s)) => Some(s.as_str()),
            _ => None,
        }).collect();
        assert!(codes.contains(&"nbformat-cell-type"));
        assert!(codes.contains(&"nbformat-cell-id"));
        // the bad cell diagnostic is anchored at its own line
        let bad = diags.iter().find(|d| d.message.contains("invalid cell_type")).unwrap();
        assert_eq!(bad.range.start.line, 3);
    }
}
