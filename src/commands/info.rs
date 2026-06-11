use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

pub const FEAT_SYNC: u32 = 0x0000_0001;
pub const FEAT_RESET: u32 = 0x0000_0002;

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub firmware: String,
    pub protocol: u8,
}

#[derive(Debug, Serialize)]
pub struct CapsInfo {
    pub proto_ver: u8,
    pub max_dio: u8,
    pub out_count: u8,
    pub in_count: u8,
    pub ts_resolution_ns: u32,
    pub max_pattern_steps: u16,
    pub ts_ring_events: u16,
    pub features: u32,
}

impl CapsInfo {
    pub fn has_sync(&self) -> bool {
        self.features & FEAT_SYNC != 0
    }

    pub fn has_reset(&self) -> bool {
        self.features & FEAT_RESET != 0
    }

    pub fn feature_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.has_sync() {
            labels.push("sync");
        }
        if self.has_reset() {
            labels.push("reset");
        }
        labels
    }
}

#[derive(Debug, Serialize)]
pub struct DeviceInfoResult {
    pub uid: String,
    pub device_number: Option<u8>,
}

pub fn run_version(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "version")?;
    let data = resp.ok_data()?;
    let parsed = parse_version(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Firmware: {}", parsed.firmware);
        println!("Protocol: v{}", parsed.protocol);
    }
    Ok(())
}

pub fn run_caps(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "caps")?;
    let data = resp.ok_data()?;
    let parsed = parse_caps(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Protocol version: {}", parsed.proto_ver);
        println!("Max DIO channels:  {}", parsed.max_dio);
        println!("Output channels:   {}", parsed.out_count);
        println!("Input channels:    {}", parsed.in_count);
        println!(
            "TS resolution:     {} ns (sub-µs phase step)",
            parsed.ts_resolution_ns
        );
        println!("Max pattern steps: {}", parsed.max_pattern_steps);
        println!("TS ring capacity:  {}", parsed.ts_ring_events);
        let labels = parsed.feature_labels();
        if labels.is_empty() {
            println!("Features:          {:#010x}", parsed.features);
        } else {
            println!(
                "Features:          {:#010x} ({})",
                parsed.features,
                labels.join(", ")
            );
        }
    }
    Ok(())
}

pub fn run_info(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "info")?;
    let data = resp.ok_data()?;
    let parsed = parse_device_info(data)?;

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("UID:           {}", parsed.uid);
        match parsed.device_number {
            Some(n) => println!("Device number: {n}"),
            None => println!("Device number: (unassigned)"),
        }
    }
    Ok(())
}

pub fn run_devnum(selector: &DeviceSelector, value: &str, json: bool) -> Result<()> {
    if value == "auto" {
        return run_devnum_auto(json);
    }

    let num: u16 = if value == "reset" {
        255
    } else {
        value.parse().context("invalid device number")?
    };

    if num > 255 {
        bail!("device number must be 0-254 (or 'reset')");
    }

    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, &format!("devnum {num}"))?;
    resp.ok_data()?;

    if json {
        let result = if num == 255 {
            serde_json::json!({"device_number": serde_json::Value::Null})
        } else {
            serde_json::json!({"device_number": num})
        };
        output::print_json(&result)?;
    } else if num == 255 {
        println!("Device number reset to unassigned.");
    } else {
        println!("Device number set to {num}.");
    }
    Ok(())
}

fn run_devnum_auto(json: bool) -> Result<()> {
    let devices = device::enumerate_devices()?;
    if devices.is_empty() {
        bail!("no Pochi-DIO devices found");
    }

    let mut used: std::collections::HashSet<u8> = std::collections::HashSet::new();
    for d in &devices {
        if let Some(n) = d.device_number {
            used.insert(n);
        }
    }

    let unassigned: Vec<_> = devices
        .iter()
        .filter(|d| d.device_number.is_none())
        .collect();
    if unassigned.is_empty() {
        if json {
            output::print_json(&serde_json::json!({"assignments": []}))?;
        } else {
            println!("All devices already have assigned numbers.");
        }
        return Ok(());
    }

    let mut assignments = Vec::new();
    let mut next_num: u8 = 0;

    for dev in &unassigned {
        while used.contains(&next_num) {
            next_num = next_num
                .checked_add(1)
                .context("no available device numbers")?;
        }

        let mut port = transport::open_port(&dev.port)?;
        let resp = transport::send_command(&mut port, &format!("devnum {next_num}"))?;
        resp.ok_data()?;

        assignments.push(serde_json::json!({
            "uid": dev.uid,
            "port": dev.port,
            "device_number": next_num,
        }));

        used.insert(next_num);
        next_num = next_num.saturating_add(1);
    }

    if json {
        output::print_json(&serde_json::json!({"assignments": assignments}))?;
    } else {
        for a in &assignments {
            println!(
                "Assigned device {} ({}) → #{}",
                a["uid"].as_str().unwrap_or(""),
                a["port"].as_str().unwrap_or(""),
                a["device_number"]
            );
        }
    }
    Ok(())
}

pub fn parse_version(data: &str) -> Result<VersionInfo> {
    let mut firmware = String::new();
    let mut protocol = 0u8;

    for part in data.split_whitespace() {
        if let Some(val) = part.strip_prefix("proto=") {
            protocol = val.parse().context("invalid proto")?;
        } else if part.starts_with('v') || part.contains('.') {
            firmware = part.to_string();
        }
    }

    if firmware.is_empty() {
        bail!("missing firmware version in response: {data}");
    }

    Ok(VersionInfo { firmware, protocol })
}

pub fn parse_caps(data: &str) -> Result<CapsInfo> {
    let mut info = CapsInfo {
        proto_ver: 0,
        max_dio: 0,
        out_count: 0,
        in_count: 0,
        ts_resolution_ns: 0,
        max_pattern_steps: 0,
        ts_ring_events: 0,
        features: 0,
    };

    for part in data.split_whitespace() {
        if let Some((key, val)) = part.split_once('=') {
            match key {
                "dio" => info.max_dio = val.parse().context("invalid dio")?,
                "out" => info.out_count = val.parse().context("invalid out")?,
                "in" => info.in_count = val.parse().context("invalid in")?,
                "ts_res" => {
                    let ns_str = val.strip_suffix("ns").unwrap_or(val);
                    info.ts_resolution_ns = ns_str.parse().context("invalid ts_res")?;
                }
                "pattern" => {
                    info.max_pattern_steps = val.parse().context("invalid pattern")?;
                }
                "ring" => info.ts_ring_events = val.parse().context("invalid ring")?,
                "feat" => {
                    info.features = parse_hex_or_dec(val).context("invalid feat")? as u32;
                }
                _ => {}
            }
        }
    }

    Ok(info)
}

pub fn parse_device_info(data: &str) -> Result<DeviceInfoResult> {
    let mut uid = String::new();
    let mut devnum: Option<u8> = None;

    for part in data.split_whitespace() {
        if let Some(val) = part.strip_prefix("uid=") {
            uid = val.to_string();
        } else if let Some(val) = part.strip_prefix("devnum=") {
            let n: u16 = val.parse().context("invalid devnum")?;
            devnum = if n == 255 { None } else { Some(n as u8) };
        }
    }

    if uid.is_empty() {
        bail!("missing uid in info response");
    }

    Ok(DeviceInfoResult {
        uid,
        device_number: devnum,
    })
}

fn parse_hex_or_dec(s: &str) -> Result<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).context("invalid hex")
    } else {
        s.parse::<u64>().context("invalid number")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_basic() {
        let v = parse_version("v0.1.0 proto=1").unwrap();
        assert_eq!(v.firmware, "v0.1.0");
        assert_eq!(v.protocol, 1);
    }

    #[test]
    fn parse_caps_basic() {
        let c =
            parse_caps("dio=23 out=12 in=11 ts_res=1000ns pattern=256 ring=256 feat=0x00000001")
                .unwrap();
        assert_eq!(c.max_dio, 23);
        assert_eq!(c.out_count, 12);
        assert_eq!(c.in_count, 11);
        assert_eq!(c.ts_resolution_ns, 1000);
        assert_eq!(c.max_pattern_steps, 256);
        assert_eq!(c.ts_ring_events, 256);
        assert_eq!(c.features, 1);
    }

    #[test]
    fn parse_caps_zero_outputs() {
        let c = parse_caps("dio=23 out=0 in=23 ts_res=1000ns pattern=256 ring=256 feat=0x00000001")
            .unwrap();
        assert_eq!(c.out_count, 0);
        assert_eq!(c.in_count, 23);
    }

    #[test]
    fn caps_feature_helpers() {
        let only_sync =
            parse_caps("dio=23 out=0 in=23 ts_res=40ns pattern=256 ring=256 feat=0x00000001")
                .unwrap();
        assert!(only_sync.has_sync());
        assert!(!only_sync.has_reset());
        assert_eq!(only_sync.feature_labels(), vec!["sync"]);

        let sync_and_reset =
            parse_caps("dio=23 out=0 in=23 ts_res=40ns pattern=256 ring=256 feat=0x00000003")
                .unwrap();
        assert!(sync_and_reset.has_sync());
        assert!(sync_and_reset.has_reset());
        assert_eq!(sync_and_reset.feature_labels(), vec!["sync", "reset"]);

        let none =
            parse_caps("dio=23 out=0 in=23 ts_res=40ns pattern=256 ring=256 feat=0x00000000")
                .unwrap();
        assert!(!none.has_sync());
        assert!(!none.has_reset());
        assert!(none.feature_labels().is_empty());
    }

    #[test]
    fn parse_device_info_basic() {
        let d = parse_device_info("uid=AABBCCDD11223344 devnum=42").unwrap();
        assert_eq!(d.uid, "AABBCCDD11223344");
        assert_eq!(d.device_number, Some(42));
    }

    #[test]
    fn parse_device_info_unassigned() {
        let d = parse_device_info("uid=AABBCCDD11223344 devnum=255").unwrap();
        assert_eq!(d.device_number, None);
    }

    #[test]
    fn parse_hex_or_dec_works() {
        assert_eq!(parse_hex_or_dec("42").unwrap(), 42);
        assert_eq!(parse_hex_or_dec("0xFF").unwrap(), 255);
        assert_eq!(parse_hex_or_dec("0x00000001").unwrap(), 1);
    }
}
