use anyhow::{Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

#[derive(Debug, Serialize)]
pub struct MapInfo {
    pub version: u8,
    pub out_base: u8,
    pub out_count: u8,
    pub in_base: u8,
    pub in_count: u8,
    pub pull: String,
    pub flags: String,
    pub out_default: String,
    pub out_polarity: String,
    pub in_polarity: String,
}

pub fn run_show(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "map get")?;
    let data = resp.ok_data()?;
    let parsed = parse_map_response(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Map version: {}", parsed.version);
        if parsed.out_count > 0 {
            println!(
                "Output: GP{}..GP{} ({} ch)",
                parsed.out_base,
                parsed.out_base + parsed.out_count - 1,
                parsed.out_count
            );
            println!("  Default:  {}", parsed.out_default);
            println!("  Polarity: {}", parsed.out_polarity);
        } else {
            println!("Output: none");
        }
        if parsed.in_count > 0 {
            println!(
                "Input:  GP{}..GP{} ({} ch)",
                parsed.in_base,
                parsed.in_base + parsed.in_count - 1,
                parsed.in_count
            );
            println!("  Pull: {}", parsed.pull);
            println!("  Polarity: {}", parsed.in_polarity);
        } else {
            println!("Input:  none");
        }
        println!("Flags: {}", parsed.flags);
    }
    Ok(())
}

pub fn run_set(selector: &DeviceSelector, hex: &str, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("map set {hex}");
    let resp = transport::send_command(&mut port, &cmd)?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Map set.");
    }
    Ok(())
}

pub fn run_save(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "map save")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Map saved to flash.");
    }
    Ok(())
}

pub fn run_default(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "map default")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Default map loaded (all 23 channels input).");
    }
    Ok(())
}

pub fn run_load(selector: &DeviceSelector, file: &str, json: bool) -> Result<()> {
    let contents = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read map file: {file}"))?;
    let hex = crate::toml_convert::map_toml_to_hex(&contents)?;

    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("map set {hex}");
    let resp = transport::send_command(&mut port, &cmd)?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok", "hex": hex}))?;
    } else {
        println!("Map loaded from {file}.");
    }
    Ok(())
}

pub fn parse_map_response(data: &str) -> Result<MapInfo> {
    let mut info = MapInfo {
        version: 1,
        out_base: 0,
        out_count: 0,
        in_base: 0,
        in_count: 0,
        pull: String::new(),
        flags: String::new(),
        out_default: String::new(),
        out_polarity: String::new(),
        in_polarity: String::new(),
    };

    for part in data.split_whitespace() {
        if let Some((key, val)) = part.split_once('=') {
            match key {
                "v" => info.version = val.parse().context("invalid version")?,
                "out" => {
                    if let Some((base, count)) = val.split_once(':') {
                        info.out_base = base.parse().context("invalid out_base")?;
                        info.out_count = count.parse().context("invalid out_count")?;
                    }
                }
                "in" => {
                    if let Some((base, count)) = val.split_once(':') {
                        info.in_base = base.parse().context("invalid in_base")?;
                        info.in_count = count.parse().context("invalid in_count")?;
                    }
                }
                "pull" => info.pull = val.to_string(),
                "flags" => info.flags = val.to_string(),
                _ => {}
            }
        }
    }

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_map_basic() {
        let m = parse_map_response("v=1 out=0:12 in=12:11 pull=up flags=0x01").unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.out_base, 0);
        assert_eq!(m.out_count, 12);
        assert_eq!(m.in_base, 12);
        assert_eq!(m.in_count, 11);
        assert_eq!(m.pull, "up");
        assert_eq!(m.flags, "0x01");
    }

    #[test]
    fn parse_map_all_input() {
        let m = parse_map_response("v=1 out=0:0 in=0:23 pull=down flags=0x01").unwrap();
        assert_eq!(m.out_count, 0);
        assert_eq!(m.in_count, 23);
    }
}
