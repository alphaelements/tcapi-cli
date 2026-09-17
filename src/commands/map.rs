use anyhow::{bail, Context, Result};
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

/// PinMapV2 as reported by the text-mode `map get` response.
///
/// The text protocol only carries `direction_mask` (derived from the
/// per-channel `chN: OUT/IN` lines) plus `flags`; `pull_*`, `*_polarity`,
/// `out_default`, and `edge_mask` are binary-mode-only fields and are not
/// available here (see design doc §3 D2, §7 risk 3).
#[derive(Debug, Serialize)]
pub struct MapInfoV2 {
    pub version: u8,
    pub flags: String,
    pub direction_mask: u32,
    pub out_count: u8,
    pub in_count: u8,
    pub channels: Vec<String>,
}

/// Result of parsing a `map get` response: v1 (24B) or v2 (32B) pin map.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MapResult {
    V1(MapInfo),
    V2(MapInfoV2),
}

/// Number of logical channels reported by v2 `map get` (ch0..ch24).
pub const MAP_V2_CHANNEL_LINES: usize = 25;

/// Maps a logical v2 channel index to its GPIO pin number.
/// ch0-22 -> GP0-22, ch23 -> GP26, ch24 -> GP27 (see design doc §2.3).
fn logical_ch_to_gp(ch: usize) -> Option<u8> {
    match ch {
        0..=22 => Some(ch as u8),
        23 => Some(26),
        24 => Some(27),
        _ => None,
    }
}

pub fn run_show(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let (resp, channel_lines) = transport::send_command_map_get(&mut port)?;
    let data = resp.ok_data()?;
    let parsed = parse_map_response(data, &channel_lines.unwrap_or_default())?;

    if json {
        output::print_json(&parsed)?;
    } else {
        match &parsed {
            MapResult::V1(info) => print_map_v1(info),
            MapResult::V2(info) => print_map_v2(info),
        }
    }
    Ok(())
}

fn print_map_v1(parsed: &MapInfo) {
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

fn print_map_v2(parsed: &MapInfoV2) {
    println!("Map version: {}", parsed.version);
    println!(
        "Direction mask: 0x{:08X} (out={}, in={})",
        parsed.direction_mask, parsed.out_count, parsed.in_count
    );
    for (ch, dir) in parsed.channels.iter().enumerate() {
        println!("  ch{ch}: {dir}");
    }
    println!("Flags: {}", parsed.flags);
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
        println!("Default map loaded (all 25 channels input).");
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

/// Parses a `map get` response.
///
/// `data` is the header line payload (after `OK `). When the header reports
/// `v=2`, `channel_lines` must contain the 25 `chN: OUT`/`chN: IN` detail
/// lines that follow the header in the same response (see design doc §2.4,
/// §3 D1); it is ignored for v1 responses.
pub fn parse_map_response(data: &str, channel_lines: &[String]) -> Result<MapResult> {
    let version = data
        .split_whitespace()
        .find_map(|part| part.strip_prefix("v="))
        .context("missing version field")?
        .parse::<u8>()
        .context("invalid version")?;

    if version >= 2 {
        Ok(MapResult::V2(parse_map_response_v2(data, channel_lines)?))
    } else {
        Ok(MapResult::V1(parse_map_response_v1(data)?))
    }
}

fn parse_map_response_v1(data: &str) -> Result<MapInfo> {
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

fn parse_map_response_v2(data: &str, channel_lines: &[String]) -> Result<MapInfoV2> {
    let mut version = 2u8;
    let mut out_count = 0u8;
    let mut in_count = 0u8;
    let mut flags = String::new();

    for part in data.split_whitespace() {
        if let Some((key, val)) = part.split_once('=') {
            match key {
                "v" => version = val.parse().context("invalid version")?,
                "out" => out_count = val.parse().context("invalid out count")?,
                "in" => in_count = val.parse().context("invalid in count")?,
                "flags" => flags = val.to_string(),
                _ => {}
            }
        }
    }

    if channel_lines.len() != MAP_V2_CHANNEL_LINES {
        bail!(
            "expected {MAP_V2_CHANNEL_LINES} channel lines for v2 map, got {}",
            channel_lines.len()
        );
    }

    let mut channels = Vec::with_capacity(MAP_V2_CHANNEL_LINES);
    let mut direction_mask = 0u32;
    for (ch, line) in channel_lines.iter().enumerate() {
        let trimmed = line.trim();
        let (_, dir) = trimmed
            .split_once(':')
            .with_context(|| format!("malformed channel line: {trimmed}"))?;
        let dir = dir.trim();
        if dir == "OUT" {
            let gp = logical_ch_to_gp(ch)
                .with_context(|| format!("channel index out of range: {ch}"))?;
            direction_mask |= 1u32 << gp;
        } else if dir != "IN" {
            bail!("unrecognized channel direction: {dir}");
        }
        channels.push(dir.to_string());
    }

    Ok(MapInfoV2 {
        version,
        flags,
        direction_mask,
        out_count,
        in_count,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_map(m: MapResult) -> MapInfo {
        match m {
            MapResult::V1(info) => info,
            MapResult::V2(_) => panic!("expected v1 map result"),
        }
    }

    fn v2_map(m: MapResult) -> MapInfoV2 {
        match m {
            MapResult::V2(info) => info,
            MapResult::V1(_) => panic!("expected v2 map result"),
        }
    }

    /// Builds 25 `chN: OUT`/`chN: IN` lines from a direction bitmask over
    /// GPIO numbers (matching FW's `logical_ch_to_gp` mapping).
    fn channel_lines_from_direction_mask(direction_mask: u32) -> Vec<String> {
        (0..MAP_V2_CHANNEL_LINES)
            .map(|ch| {
                let gp = logical_ch_to_gp(ch).unwrap();
                let dir = if direction_mask & (1u32 << gp) != 0 {
                    "OUT"
                } else {
                    "IN"
                };
                format!("  ch{ch}: {dir}")
            })
            .collect()
    }

    #[test]
    fn parse_map_basic() {
        let m =
            v1_map(parse_map_response("v=1 out=0:12 in=12:11 pull=up flags=0x01", &[]).unwrap());
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
        let m =
            v1_map(parse_map_response("v=1 out=0:0 in=0:23 pull=down flags=0x01", &[]).unwrap());
        assert_eq!(m.out_count, 0);
        assert_eq!(m.in_count, 23);
    }

    #[test]
    fn parse_map_v2_basic() {
        // out=0..3 (GP0-3) are OUT, rest IN: direction_mask = 0x0F
        let lines = channel_lines_from_direction_mask(0x0F);
        let m = v2_map(parse_map_response("v=2 out=4 in=21 flags=0x01", &lines).unwrap());
        assert_eq!(m.version, 2);
        assert_eq!(m.out_count, 4);
        assert_eq!(m.in_count, 21);
        assert_eq!(m.flags, "0x01");
        assert_eq!(m.direction_mask, 0x0F);
        assert_eq!(m.channels.len(), 25);
        assert_eq!(m.channels[0], "OUT");
        assert_eq!(m.channels[4], "IN");
    }

    #[test]
    fn parse_map_v2_all_output() {
        let lines = channel_lines_from_direction_mask(0x0C7F_FFFF);
        let m = v2_map(parse_map_response("v=2 out=25 in=0 flags=0x00", &lines).unwrap());
        assert_eq!(m.out_count, 25);
        assert_eq!(m.in_count, 0);
        assert_eq!(m.direction_mask, 0x0C7F_FFFF);
        assert!(m.channels.iter().all(|c| c == "OUT"));
    }

    #[test]
    fn parse_map_v2_all_input() {
        let lines = channel_lines_from_direction_mask(0);
        let m = v2_map(parse_map_response("v=2 out=0 in=25 flags=0x00", &lines).unwrap());
        assert_eq!(m.out_count, 0);
        assert_eq!(m.in_count, 25);
        assert_eq!(m.direction_mask, 0);
        assert!(m.channels.iter().all(|c| c == "IN"));
    }

    #[test]
    fn parse_map_v2_ch23_ch24_map_to_gp26_gp27() {
        // Only ch23 (GP26) and ch24 (GP27) are OUT.
        let direction_mask = (1u32 << 26) | (1u32 << 27);
        let lines = channel_lines_from_direction_mask(direction_mask);
        let m = v2_map(parse_map_response("v=2 out=2 in=23 flags=0x00", &lines).unwrap());
        assert_eq!(m.direction_mask, direction_mask);
        assert_eq!(m.channels[23], "OUT");
        assert_eq!(m.channels[24], "OUT");
        assert!(m.channels[..23].iter().all(|c| c == "IN"));
    }

    #[test]
    fn parse_map_v2_missing_channel_lines_errors() {
        let err = parse_map_response("v=2 out=4 in=21 flags=0x01", &[]).unwrap_err();
        assert!(err.to_string().contains("25"));
    }

    #[test]
    fn parse_map_v2_wrong_channel_line_count_errors() {
        let lines = vec!["  ch0: OUT".to_string(); 10];
        let err = parse_map_response("v=2 out=4 in=21 flags=0x01", &lines).unwrap_err();
        assert!(err.to_string().contains("25"));
    }

    #[test]
    fn parse_map_missing_version_errors() {
        let err = parse_map_response("out=0:12 in=12:11 pull=up flags=0x01", &[]).unwrap_err();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn map_v2_json_includes_v1_compatible_and_v2_specific_fields() {
        // R-07: v2 JSON must keep version/out_count/in_count/flags (v1
        // compatible field names) and add direction_mask/channels.
        let lines = channel_lines_from_direction_mask(0x0F);
        let parsed = parse_map_response("v=2 out=4 in=21 flags=0x01", &lines).unwrap();
        let json = serde_json::to_value(&parsed).unwrap();
        assert_eq!(json["version"], 2);
        assert_eq!(json["out_count"], 4);
        assert_eq!(json["in_count"], 21);
        assert_eq!(json["flags"], "0x01");
        assert_eq!(json["direction_mask"], 0x0F);
        assert_eq!(json["channels"].as_array().unwrap().len(), 25);
    }
}
