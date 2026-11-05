//! Output rendering: the [`Render`] trait and a small column table helper.
//!
//! Every command returns a value that implements [`Render`], so the dispatch
//! layer can print it either human-readably (`--json` absent) or as a single
//! JSON document on stdout (`--json`). Errors never go through here: they are
//! printed to stderr by [`crate::run`].

use std::io::{self, Write};

use serde_json::Value;

/// A value that can render itself for a terminal and as JSON.
pub trait Render {
    /// Human-readable representation (may span several lines; no trailing
    /// newline required). An empty string prints nothing.
    fn human(&self) -> String;

    /// Machine-readable JSON representation.
    fn json(&self) -> Value;
}

/// Serialize `value` to a `Value` for the default [`Render::json`] impl.
///
/// Serialization of these plain data types cannot fail; the fallback keeps the
/// function total without an `unwrap`.
pub fn to_json_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Print one rendered value to `out`. In `json` mode exactly one JSON document
/// (compact, one line) is written; otherwise the human text plus a newline.
pub fn emit<T: Render + ?Sized>(value: &T, json: bool, out: &mut dyn Write) -> io::Result<()> {
    if json {
        let text = serde_json::to_string(&value.json())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        writeln!(out, "{text}")
    } else {
        let text = value.human();
        if text.is_empty() {
            Ok(())
        } else {
            writeln!(out, "{text}")
        }
    }
}

/// Render a left-aligned column table with a header row.
///
/// Returns an empty string for an empty row set so callers can substitute
/// their own "nothing here" message.
pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut widths: Vec<usize> = headers.iter().map(|head| head.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let mut out = String::new();
    push_row(&mut out, headers.iter().copied(), &widths);
    let separators: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    push_row(&mut out, separators.iter().map(String::as_str), &widths);
    for row in rows {
        push_row(&mut out, row.iter().map(String::as_str), &widths);
    }
    // Drop the trailing newline: `emit` adds exactly one.
    out.pop();
    out
}

/// Append one padded row (no trailing spaces on the last cell) plus a newline.
fn push_row<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>, widths: &[usize]) {
    let cells: Vec<&str> = cells.collect();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push_str("  ");
        }
        out.push_str(cell);
        if index + 1 < cells.len() {
            let pad = widths
                .get(index)
                .copied()
                .unwrap_or(0)
                .saturating_sub(cell.chars().count());
            out.push_str(&" ".repeat(pad));
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde_json::json;

    struct Sample;

    impl Render for Sample {
        fn human(&self) -> String {
            String::from("hello")
        }
        fn json(&self) -> Value {
            json!({ "ok": true })
        }
    }

    #[test]
    fn emit_picks_human_or_json() {
        let mut human = Vec::new();
        emit(&Sample, false, &mut human).expect("write human");
        assert_eq!(String::from_utf8(human).expect("utf8"), "hello\n");

        let mut machine = Vec::new();
        emit(&Sample, true, &mut machine).expect("write json");
        assert_eq!(String::from_utf8(machine).expect("utf8"), "{\"ok\":true}\n");
    }

    #[test]
    fn render_table_aligns_columns() {
        let table = render_table(
            &["A", "BB"],
            &[vec!["1".into(), "x".into()], vec!["22".into(), "yy".into()]],
        );
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "A   BB");
        assert_eq!(lines[1], "--  --");
        assert_eq!(lines[2], "1   x");
        assert_eq!(lines[3], "22  yy");
    }

    #[test]
    fn render_table_empty_is_blank() {
        assert_eq!(render_table(&["A"], &[]), "");
    }

    #[derive(Serialize)]
    struct Derive {
        ok: bool,
    }

    #[test]
    fn to_json_value_serializes() {
        assert_eq!(to_json_value(&Derive { ok: true }), json!({"ok": true}));
    }
}
