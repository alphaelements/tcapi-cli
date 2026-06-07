use anyhow::{Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

#[derive(Debug, Serialize)]
pub struct ReadResult {
    pub inputs: String,
}

#[derive(Debug, Serialize)]
pub struct StateResult {
    pub inputs: String,
    pub outputs: String,
    pub flags: String,
}

pub fn run_read(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "read")?;
    let data = resp.ok_data()?;

    if json {
        output::print_json(&ReadResult {
            inputs: data.trim().to_string(),
        })?;
    } else {
        println!("Inputs: {data}");
    }
    Ok(())
}

pub fn run_write(selector: &DeviceSelector, values: &str, mask: &str, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("write {values} {mask}");
    let resp = transport::send_command(&mut port, &cmd)?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Output set.");
    }
    Ok(())
}

pub fn run_state(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "state")?;
    let data = resp.ok_data()?;
    let parsed = parse_state(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Inputs:  {}", parsed.inputs);
        println!("Outputs: {}", parsed.outputs);
        println!("Flags:   {}", parsed.flags);
    }
    Ok(())
}

pub fn run_watch(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "subscribe on")?;
    resp.ok_data()?;

    if !json {
        println!("Watching for input changes (Ctrl+C to stop)...");
    }

    let ctrlc = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctrlc_clone = ctrlc.clone();
    ctrlc::set_handler(move || {
        ctrlc_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    })
    .context("failed to set Ctrl+C handler")?;

    let events = transport::read_push_events(&mut port)?;
    for event in events {
        if ctrlc.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        match event {
            Ok(data) => {
                if json {
                    let parsed = parse_input_event(&data);
                    if let Ok(ev) = parsed {
                        let _ = output::print_json(&ev);
                    }
                } else {
                    println!("{data}");
                }
            }
            Err(_) => break,
        }
    }

    let _ = transport::send_command(&mut port, "subscribe off");
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct InputEvent {
    pub inputs: String,
    pub timestamp: String,
}

fn parse_input_event(data: &str) -> Result<InputEvent> {
    let mut inputs = String::new();
    let mut timestamp = String::new();

    let rest = data.strip_prefix("INPUT ").unwrap_or(data);
    for part in rest.split_whitespace() {
        if let Some(val) = part.strip_prefix("t=") {
            timestamp = val.to_string();
        } else if part.starts_with("0x") || part.starts_with("0X") {
            inputs = part.to_string();
        }
    }

    Ok(InputEvent { inputs, timestamp })
}

pub fn parse_state(data: &str) -> Result<StateResult> {
    let mut inputs = String::new();
    let mut outputs = String::new();
    let mut flags = String::new();

    for part in data.split_whitespace() {
        if let Some(val) = part.strip_prefix("in=") {
            inputs = val.to_string();
        } else if let Some(val) = part.strip_prefix("out=") {
            outputs = val.to_string();
        } else if let Some(val) = part.strip_prefix("flags=") {
            flags = val.to_string();
        }
    }

    Ok(StateResult {
        inputs,
        outputs,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_state_basic() {
        let s = parse_state("in=0x00000005 out=0x0000000A flags=0x00").unwrap();
        assert_eq!(s.inputs, "0x00000005");
        assert_eq!(s.outputs, "0x0000000A");
        assert_eq!(s.flags, "0x00");
    }

    #[test]
    fn parse_input_event_basic() {
        let e = parse_input_event("INPUT 0x000005 t=1234567890").unwrap();
        assert_eq!(e.inputs, "0x000005");
        assert_eq!(e.timestamp, "1234567890");
    }
}
