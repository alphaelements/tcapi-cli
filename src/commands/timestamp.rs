use anyhow::{Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

#[derive(Debug, Serialize)]
pub struct TimestampEvent {
    pub inputs: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct TimestampResult {
    pub count: u16,
    pub events: Vec<TimestampEvent>,
}

pub fn run_arm(selector: &DeviceSelector, mask: &str, mode: &str, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("ts arm {mask} {mode}");
    let resp = transport::send_command(&mut port, &cmd)?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Timestamp armed (mask={mask}, mode={mode}).");
    }
    Ok(())
}

pub fn run_get(selector: &DeviceSelector, max_events: u16, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("ts get {max_events}");
    let lines = transport::send_command_raw(&mut port, &cmd)?;
    let parsed = parse_ts_response(&lines)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Timestamp events: {}", parsed.count);
        for (i, ev) in parsed.events.iter().enumerate() {
            println!("  [{i}] inputs={} t={}µs", ev.inputs, ev.timestamp);
        }
    }
    Ok(())
}

pub fn run_disarm(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "ts disarm")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Timestamp disarmed.");
    }
    Ok(())
}

pub fn run_capture(
    selector: &DeviceSelector,
    mask: &str,
    mode: &str,
    max_events: u16,
    json: bool,
) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;

    let arm_cmd = format!("ts arm {mask} {mode}");
    let resp = transport::send_command(&mut port, &arm_cmd)?;
    resp.ok_data()?;

    if !json {
        println!("Armed. Waiting for events (Ctrl+C to stop)...");
    }

    std::thread::sleep(std::time::Duration::from_secs(1));

    let get_cmd = format!("ts get {max_events}");
    let lines = transport::send_command_raw(&mut port, &get_cmd)?;
    let parsed = parse_ts_response(&lines)?;

    let _ = transport::send_command(&mut port, "ts disarm");

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Captured {} events:", parsed.count);
        for (i, ev) in parsed.events.iter().enumerate() {
            println!("  [{i}] inputs={} t={}µs", ev.inputs, ev.timestamp);
        }
    }
    Ok(())
}

pub fn parse_ts_response(lines: &[String]) -> Result<TimestampResult> {
    let mut events = Vec::new();
    let mut count = 0u16;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("OK") {
            if let Some(rest) = trimmed.strip_prefix("OK") {
                let rest = rest.trim();
                if let Some(val) = rest.strip_prefix("count=") {
                    count = val.parse().context("invalid count")?;
                }
            }
        } else if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let inputs = parts[0].to_string();
                let ts_str = parts[1].strip_prefix("t=").unwrap_or(parts[1]);
                let timestamp: u64 = ts_str.parse().context("invalid timestamp")?;
                events.push(TimestampEvent { inputs, timestamp });
            }
        }
    }

    if count == 0 {
        count = events.len() as u16;
    }

    Ok(TimestampResult { count, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ts_response_basic() {
        let lines = vec![
            "OK count=2".to_string(),
            "  0x00000005 t=1000".to_string(),
            "  0x0000000A t=2000".to_string(),
        ];
        let r = parse_ts_response(&lines).unwrap();
        assert_eq!(r.count, 2);
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.events[0].inputs, "0x00000005");
        assert_eq!(r.events[0].timestamp, 1000);
        assert_eq!(r.events[1].inputs, "0x0000000A");
        assert_eq!(r.events[1].timestamp, 2000);
    }

    #[test]
    fn parse_ts_response_empty() {
        let lines = vec!["OK count=0".to_string()];
        let r = parse_ts_response(&lines).unwrap();
        assert_eq!(r.count, 0);
        assert!(r.events.is_empty());
    }
}
