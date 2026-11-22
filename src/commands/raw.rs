use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};

use tcapi_sdk::Connection;

use crate::device::{self, DeviceSelector};
use crate::output;

pub fn run_raw(selector: &DeviceSelector, text: &str, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    raw(&mut conn, text, json, &mut io::stdout().lock())
}

/// Send `text` and print **every** line of the response: the header and, for
/// a counted response, all of its continuation lines. A device `ERR` is a
/// line, not a failure.
pub fn raw(conn: &mut Connection, text: &str, json: bool, out: &mut impl Write) -> Result<()> {
    let lines = conn.raw(text)?;

    if json {
        output::write_json(out, &serde_json::json!({"lines": lines}))?;
    } else {
        for line in &lines {
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}

pub fn run_interactive(selector: &DeviceSelector) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    println!("Tcapi-DIO interactive mode (type 'exit' or Ctrl+D to quit)");
    repl(
        io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        |cmd| Ok(conn.raw(cmd)?),
    )
}

/// Read commands from `input`, run each through `exec` and print **every**
/// response line to `out`. A failing command is reported on `err` and the
/// session goes on; `exit`, `quit` and end of input end it.
pub fn repl(
    mut input: impl BufRead,
    out: &mut impl Write,
    err: &mut impl Write,
    mut exec: impl FnMut(&str) -> Result<Vec<String>>,
) -> Result<()> {
    loop {
        write!(out, "tcapi> ").context("failed to write prompt")?;
        out.flush().context("failed to flush stdout")?;

        let mut line = String::new();
        let n = input.read_line(&mut line).context("failed to read input")?;
        if n == 0 {
            writeln!(out).context("failed to write output")?;
            return Ok(());
        }

        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }
        if cmd == "exit" || cmd == "quit" {
            return Ok(());
        }

        match exec(cmd) {
            Ok(lines) => {
                for response in &lines {
                    writeln!(out, "{response}").context("failed to write output")?;
                }
            }
            Err(e) => writeln!(err, "Error: {e:#}").context("failed to write error")?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str, exec: impl FnMut(&str) -> Result<Vec<String>>) -> (String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        repl(input.as_bytes(), &mut out, &mut err, exec).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn repl_prints_every_line_of_a_multi_line_response() {
        let (out, err) = run("help\nexit\n", |cmd| {
            assert_eq!(cmd, "help");
            Ok(vec![
                "OK count=2".to_string(),
                "  read".to_string(),
                "  write".to_string(),
            ])
        });
        assert_eq!(out, "tcapi> OK count=2\n  read\n  write\ntcapi> ");
        assert!(err.is_empty());
    }

    #[test]
    fn repl_reports_errors_and_continues() {
        let mut calls = 0;
        let (out, err) = run("bad\nok\n", |cmd| {
            calls += 1;
            if cmd == "bad" {
                anyhow::bail!("boom")
            }
            Ok(vec!["OK".to_string()])
        });
        assert_eq!(calls, 2);
        assert!(out.contains("OK\n"));
        assert_eq!(err, "Error: boom\n");
    }

    #[test]
    fn repl_skips_blank_lines_and_stops_at_eof() {
        let mut calls = 0;
        let (out, _) = run("\n   \n", |_| {
            calls += 1;
            Ok(vec![])
        });
        assert_eq!(calls, 0);
        assert!(out.ends_with("tcapi> \n"));
    }

    #[test]
    fn repl_quit_ends_the_session_without_running_it() {
        let mut calls = 0;
        run("quit\nread\n", |_| {
            calls += 1;
            Ok(vec![])
        });
        assert_eq!(calls, 0);
    }
}
