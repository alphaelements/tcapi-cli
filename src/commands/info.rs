use std::io::{self, Write};

use anyhow::{bail, Context, Result};
use tcapi_sdk::{Capabilities, Connection, DeviceInfo, VersionInfo};

use crate::device::{self, DeviceSelector};
use crate::output;

/// The `devnum` value that clears the assignment on the device.
const DEVNUM_UNASSIGNED: u8 = 255;

/// Names of the feature bits set in `caps.features`.
pub fn feature_labels(caps: &Capabilities) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if caps.has_sync() {
        labels.push("sync");
    }
    if caps.has_reset() {
        labels.push("reset");
    }
    labels
}

/// Firmware version as `v<major>.<minor>.<patch>`.
pub fn format_firmware(version: &VersionInfo) -> String {
    format!("v{}.{}.{}", version.major, version.minor, version.patch)
}

pub fn run_version(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    version(&mut conn, json, &mut io::stdout().lock())
}

pub fn version(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let version = conn.version()?;

    if json {
        output::write_json(out, &version)?;
    } else {
        writeln!(out, "Firmware: {}", format_firmware(&version))?;
        writeln!(out, "Protocol: v{}", version.protocol_version)?;
    }
    Ok(())
}

pub fn run_caps(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    caps(&mut conn, json, &mut io::stdout().lock())
}

pub fn caps(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let caps = conn.caps()?;

    if json {
        output::write_json(out, &caps)?;
    } else {
        writeln!(out, "DIO channels:      {}", caps.dio)?;
        writeln!(out, "Output channels:   {}", caps.out)?;
        writeln!(out, "Input channels:    {}", caps.r#in)?;
        writeln!(
            out,
            "TS resolution:     {} ns (sub-µs phase step)",
            caps.ts_res_ns
        )?;
        writeln!(out, "Pattern tick:      {} ns", caps.tick_ns)?;
        writeln!(out, "Max pattern steps: {}", caps.pattern_max)?;
        writeln!(out, "TS ring capacity:  {}", caps.ring)?;
        let labels = feature_labels(&caps);
        if labels.is_empty() {
            writeln!(out, "Features:          {:#010x}", caps.features)?;
        } else {
            writeln!(
                out,
                "Features:          {:#010x} ({})",
                caps.features,
                labels.join(", ")
            )?;
        }
    }
    Ok(())
}

pub fn run_info(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    info(&mut conn, json, &mut io::stdout().lock())
}

pub fn info(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let info = conn.info()?;

    if json {
        output::write_json(out, &info)?;
    } else {
        writeln!(out, "UID:           {}", info.uid)?;
        match info.device_number {
            Some(n) => writeln!(out, "Device number: {n}")?,
            None => writeln!(out, "Device number: (unassigned)")?,
        }
    }
    Ok(())
}

/// Parse the `devnum` argument: `reset` clears the number (255), otherwise a
/// number 0-254.
pub fn parse_devnum(value: &str) -> Result<u8> {
    if value == "reset" {
        return Ok(DEVNUM_UNASSIGNED);
    }
    let num: u8 = value
        .parse()
        .with_context(|| format!("invalid device number {value:?} (0-254, 'reset' or 'auto')"))?;
    if num == DEVNUM_UNASSIGNED {
        bail!("device number must be 0-254 (use 'reset' to unassign)");
    }
    Ok(num)
}

pub fn run_devnum(selector: &DeviceSelector, value: &str, json: bool) -> Result<()> {
    if value == "auto" {
        return run_devnum_auto(json);
    }
    let num = parse_devnum(value)?;

    let (_info, mut conn) = device::open_device(selector)?;
    conn.set_device_number(num)?;

    if json {
        let number = (num != DEVNUM_UNASSIGNED).then_some(num);
        output::print_json(&serde_json::json!({ "device_number": number }))?;
    } else if num == DEVNUM_UNASSIGNED {
        println!("Device number reset to unassigned.");
    } else {
        println!("Device number set to {num}.");
    }
    Ok(())
}

/// Pair every unassigned device (by index into `devices`) with the lowest
/// number not used by any device, ascending.
pub fn plan_auto_assignment(devices: &[DeviceInfo]) -> Result<Vec<(usize, u8)>> {
    let mut used: std::collections::HashSet<u8> =
        devices.iter().filter_map(|d| d.device_number).collect();
    let mut plan = Vec::new();
    for (index, dev) in devices.iter().enumerate() {
        if dev.device_number.is_some() {
            continue;
        }
        let free = (0..DEVNUM_UNASSIGNED)
            .find(|n| !used.contains(n))
            .context("no available device numbers")?;
        used.insert(free);
        plan.push((index, free));
    }
    Ok(plan)
}

fn run_devnum_auto(json: bool) -> Result<()> {
    let devices = device::enumerate_devices()?;
    if devices.is_empty() {
        bail!("no Tcapi-DIO devices found");
    }

    let plan = plan_auto_assignment(&devices)?;
    if plan.is_empty() {
        if json {
            output::print_json(&serde_json::json!({"assignments": []}))?;
        } else {
            println!("All devices already have assigned numbers.");
        }
        return Ok(());
    }

    let mut assignments = Vec::new();
    for (index, number) in plan {
        let dev = &devices[index];
        device::open_port(&dev.port)?.set_device_number(number)?;
        assignments.push(serde_json::json!({
            "uid": dev.uid,
            "port": dev.port,
            "device_number": number,
        }));
    }

    if json {
        output::print_json(&serde_json::json!({"assignments": assignments}))?;
    } else {
        for a in &assignments {
            println!(
                "Assigned device {} ({}) → #{}",
                a["uid"].as_str().unwrap_or_default(),
                a["port"].as_str().unwrap_or_default(),
                a["device_number"]
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tcapi_sdk::{FEAT_RESET, FEAT_SYNC};

    fn caps(features: u32) -> Capabilities {
        Capabilities {
            dio: 25,
            out: 12,
            r#in: 13,
            ts_res_ns: 40,
            tick_ns: 200,
            pattern_max: 256,
            ring: 256,
            features,
        }
    }

    fn device(devnum: Option<u8>) -> DeviceInfo {
        DeviceInfo {
            uid: "UID".into(),
            device_number: devnum,
            port: "port".into(),
            version: None,
        }
    }

    #[test]
    fn feature_labels_name_each_bit() {
        assert_eq!(feature_labels(&caps(FEAT_SYNC)), vec!["sync"]);
        assert_eq!(feature_labels(&caps(FEAT_RESET)), vec!["reset"]);
        assert_eq!(
            feature_labels(&caps(FEAT_SYNC | FEAT_RESET)),
            vec!["sync", "reset"]
        );
        assert!(feature_labels(&caps(0)).is_empty());
    }

    #[test]
    fn firmware_is_formatted_with_v_prefix() {
        let version = VersionInfo {
            major: 0,
            minor: 1,
            patch: 3,
            protocol_version: 2,
        };
        assert_eq!(format_firmware(&version), "v0.1.3");
    }

    #[test]
    fn devnum_accepts_range_and_reset() {
        assert_eq!(parse_devnum("0").unwrap(), 0);
        assert_eq!(parse_devnum("254").unwrap(), 254);
        assert_eq!(parse_devnum("reset").unwrap(), 255);
    }

    #[test]
    fn devnum_rejects_255_overflow_and_garbage() {
        assert!(parse_devnum("255").is_err());
        assert!(parse_devnum("256").is_err());
        assert!(parse_devnum("-1").is_err());
        assert!(parse_devnum("x").is_err());
    }

    #[test]
    fn auto_assignment_skips_used_numbers() {
        let devices = vec![device(Some(0)), device(None), device(Some(2)), device(None)];
        assert_eq!(
            plan_auto_assignment(&devices).unwrap(),
            vec![(1, 1), (3, 3)]
        );
    }

    #[test]
    fn auto_assignment_with_nothing_to_do_is_empty() {
        let devices = vec![device(Some(4)), device(Some(5))];
        assert!(plan_auto_assignment(&devices).unwrap().is_empty());
    }

    #[test]
    fn auto_assignment_fails_when_all_numbers_are_taken() {
        let mut devices: Vec<DeviceInfo> = (0..=254u8).map(|n| device(Some(n))).collect();
        devices.push(device(None));
        assert!(plan_auto_assignment(&devices).is_err());
    }
}
