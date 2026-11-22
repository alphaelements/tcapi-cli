use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use tcapi_sdk::{config, Connection, PatternMode, PatternState, PatternStep, PATTERN_TICK_NS};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::parse_step;

/// Lower-case name of a pattern engine state.
pub fn state_label(state: PatternState) -> &'static str {
    match state {
        PatternState::Idle => "idle",
        PatternState::Loaded => "loaded",
        PatternState::Running => "running",
        PatternState::Done => "done",
    }
}

/// Human summary of a step list: step count and total hold time. The hold of
/// each step is in pattern ticks; the per-step PIO overhead is not counted.
pub fn summarize(steps: &[PatternStep]) -> String {
    let ticks: u64 = steps.iter().map(|s| u64::from(s.hold_ticks)).sum();
    format!(
        "{} steps, {} ticks ({} ns)",
        steps.len(),
        ticks,
        ticks * u64::from(PATTERN_TICK_NS)
    )
}

pub fn run_add(selector: &DeviceSelector, steps: &[String], json: bool) -> Result<()> {
    let steps: Vec<PatternStep> = steps.iter().map(|s| parse_step(s)).collect::<Result<_>>()?;
    let (_info, mut conn) = device::open_device(selector)?;
    add(&mut conn, &steps, json, &mut io::stdout().lock())
}

/// Append `steps` to the pattern buffer (split into lines by the SDK).
pub fn add(
    conn: &mut Connection,
    steps: &[PatternStep],
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let total = conn.pattern_add(steps)?;

    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "total": total}))?;
    } else {
        writeln!(
            out,
            "Added {}. Pattern has {total} steps.",
            summarize(steps)
        )?;
    }
    Ok(())
}

/// Read and parse a `pattern.toml` with the SDK parser; errors name the file
/// and line.
pub fn load_pattern_file(file: &str) -> Result<Vec<PatternStep>> {
    let src = std::fs::read_to_string(Path::new(file))
        .with_context(|| format!("failed to read pattern file: {file}"))?;
    Ok(config::parse_pattern(&src, file)?)
}

pub fn run_load_file(selector: &DeviceSelector, file: &str, json: bool) -> Result<()> {
    let steps = load_pattern_file(file)?;
    let (_info, mut conn) = device::open_device(selector)?;
    load(&mut conn, &steps, file, json, &mut io::stdout().lock())
}

/// Replace the pattern buffer with `steps` (`pattern reset`, then add).
pub fn load(
    conn: &mut Connection,
    steps: &[PatternStep],
    source: &str,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let total = conn.pattern_load(steps)?;

    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "total": total}))?;
    } else {
        writeln!(
            out,
            "Loaded {source}: {}. The pattern now has {total} steps.",
            summarize(steps)
        )?;
    }
    Ok(())
}

pub fn run_run(selector: &DeviceSelector, loop_mode: bool, repeat: u16, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    if loop_mode {
        conn.pattern_run(PatternMode::Loop, repeat)?;
    } else {
        conn.pattern_run_once()?;
    }

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Pattern started.");
    }
    Ok(())
}

pub fn run_stop(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.pattern_stop()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Pattern stopped.");
    }
    Ok(())
}

pub fn run_reset(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.pattern_reset()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Pattern engine reset.");
    }
    Ok(())
}

pub fn run_status(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    status(&mut conn, json, &mut io::stdout().lock())
}

pub fn status(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let status = conn.pattern_status()?;

    if json {
        output::write_json(out, &status)?;
    } else {
        writeln!(out, "State:        {}", state_label(status.state))?;
        writeln!(out, "Current step: {}", status.step)?;
        writeln!(out, "Loops done:   {}", status.loops)?;
        writeln!(out, "Total steps:  {}", status.total)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_labels_are_lower_case() {
        assert_eq!(state_label(PatternState::Idle), "idle");
        assert_eq!(state_label(PatternState::Loaded), "loaded");
        assert_eq!(state_label(PatternState::Running), "running");
        assert_eq!(state_label(PatternState::Done), "done");
    }

    #[test]
    fn summary_counts_ticks_and_nanoseconds() {
        let steps = [
            PatternStep {
                out_bits: 1,
                hold_ticks: 5,
            },
            PatternStep {
                out_bits: 0,
                hold_ticks: 10,
            },
        ];
        assert_eq!(summarize(&steps), "2 steps, 15 ticks (3000 ns)");
    }

    #[test]
    fn summary_does_not_overflow_on_large_holds() {
        let steps = [PatternStep {
            out_bits: 1,
            hold_ticks: u32::MAX,
        }; 2];
        assert!(summarize(&steps).starts_with("2 steps, 8589934590 ticks"));
    }

    #[test]
    fn load_pattern_file_reports_missing_file() {
        let err = load_pattern_file("/nonexistent/tcapi-pattern.toml").unwrap_err();
        assert!(err.to_string().contains("failed to read pattern file"));
    }
}
