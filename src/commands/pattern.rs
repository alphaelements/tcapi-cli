use anyhow::{Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

#[derive(Debug, Serialize)]
pub struct PatternStatus {
    pub state: String,
    pub current_step: u16,
    pub loops_done: u16,
}

pub fn run_load_hex(selector: &DeviceSelector, hex: &str, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("pattern load {hex}");
    let resp = transport::send_command(&mut port, &cmd)?;
    let data = resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok", "response": data}))?;
    } else {
        println!("Pattern loaded. {data}");
    }
    Ok(())
}

pub fn run_load_file(selector: &DeviceSelector, file: &str, json: bool) -> Result<()> {
    let contents = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read pattern file: {file}"))?;
    let hex = crate::toml_convert::pattern_toml_to_hex(&contents)?;
    run_load_hex(selector, &hex, json)
}

pub fn run_run(selector: &DeviceSelector, loop_mode: bool, repeat: u16, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = if loop_mode {
        format!("pattern run loop {repeat}")
    } else {
        "pattern run once 1".to_string()
    };
    let resp = transport::send_command(&mut port, &cmd)?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Pattern started.");
    }
    Ok(())
}

pub fn run_stop(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "pattern stop")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Pattern stopped.");
    }
    Ok(())
}

pub fn run_status(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "pattern status")?;
    let data = resp.ok_data()?;
    let parsed = parse_pattern_status(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("State:        {}", parsed.state);
        println!("Current step: {}", parsed.current_step);
        println!("Loops done:   {}", parsed.loops_done);
    }
    Ok(())
}

pub fn parse_pattern_status(data: &str) -> Result<PatternStatus> {
    let mut state = String::new();
    let mut current_step = 0u16;
    let mut loops_done = 0u16;

    for part in data.split_whitespace() {
        if let Some((key, val)) = part.split_once('=') {
            match key {
                "state" => state = val.to_string(),
                "step" => current_step = val.parse().context("invalid step")?,
                "loops" => loops_done = val.parse().context("invalid loops")?,
                _ => {}
            }
        }
    }

    Ok(PatternStatus {
        state,
        current_step,
        loops_done,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_basic() {
        let s = parse_pattern_status("state=running step=5 loops=3").unwrap();
        assert_eq!(s.state, "running");
        assert_eq!(s.current_step, 5);
        assert_eq!(s.loops_done, 3);
    }

    #[test]
    fn parse_status_idle() {
        let s = parse_pattern_status("state=idle step=0 loops=0").unwrap();
        assert_eq!(s.state, "idle");
    }
}
