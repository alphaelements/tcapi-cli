use std::io::{self, Write};

use anyhow::{Context, Result};
use serde::Serialize;

/// Write `value` as pretty-printed JSON followed by a newline.
pub fn write_json<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    writeln!(out, "{json}").context("failed to write output")
}

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    write_json(&mut io::stdout().lock(), value)
}

/// Write a left-aligned text table with a header and a separator line.
pub fn write_table(out: &mut impl Write, headers: &[&str], rows: &[Vec<String>]) -> Result<()> {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let pad = |cells: Vec<&str>| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = widths.get(i).copied().unwrap_or(0);
                format!("{cell:width$}")
            })
            .collect::<Vec<_>>()
            .join("  ")
    };

    let header_line = pad(headers.to_vec());
    let separator = widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join("  ");
    writeln!(out, "{}", header_line.trim_end()).context("failed to write output")?;
    writeln!(out, "{separator}").context("failed to write output")?;
    for row in rows {
        let line = pad(row.iter().map(String::as_str).collect());
        writeln!(out, "{}", line.trim_end()).context("failed to write output")?;
    }
    Ok(())
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) -> Result<()> {
    write_table(&mut io::stdout().lock(), headers, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_pretty_printed_with_newline() {
        #[derive(Serialize)]
        struct T {
            x: i32,
        }
        let mut out = Vec::new();
        write_json(&mut out, &T { x: 42 }).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "{\n  \"x\": 42\n}\n");
    }

    #[test]
    fn table_aligns_columns_to_the_widest_cell() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["A", "BB"],
            &[
                vec!["long cell".to_string(), "x".to_string()],
                vec!["y".to_string(), "zz".to_string()],
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "A          BB\n---------  --\nlong cell  x\ny          zz\n"
        );
    }

    #[test]
    fn table_with_no_rows_prints_header_and_separator() {
        let mut out = Vec::new();
        write_table(&mut out, &["ONE"], &[]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "ONE\n---\n");
    }
}
