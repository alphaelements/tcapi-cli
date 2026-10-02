use std::io::{IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Result};

use pochi_sdk::DeviceInfo;

use crate::device::{self, DeviceSelector};
use crate::output;

const RESET_LATENCY: Duration = Duration::from_millis(50);

pub fn run_system_reset(selector: &DeviceSelector, yes: bool, json: bool) -> Result<()> {
    if !yes {
        confirm("Soft-reset the device? USB will disconnect and re-enumerate")?;
    }
    let (info, conn) = device::open_device(selector)?;
    // The connection is consumed: the device re-enumerates after the `OK`.
    conn.system_reset()?;

    // FW waits ~50ms after OK before issuing sys_reset. Sleep a moment so the
    // host sees a clean disconnect on the next list.
    std::thread::sleep(RESET_LATENCY);

    if json {
        output::print_json(&serde_json::json!({
            "status": "ok",
            "action": "system_reset",
            "uid": info.uid,
            "port": info.port,
        }))?;
    } else {
        println!(
            "Reset requested for {}. Device will re-enumerate in a few seconds.",
            label(&info)
        );
    }
    Ok(())
}

pub fn run_bootsel(selector: &DeviceSelector, yes: bool, json: bool) -> Result<()> {
    if !yes {
        confirm("Reboot the device into BOOTSEL mode? FW will be unloaded until you flash a UF2")?;
    }
    let (info, conn) = device::open_device(selector)?;
    conn.bootsel_reset()?;

    std::thread::sleep(RESET_LATENCY);

    if json {
        output::print_json(&serde_json::json!({
            "status": "ok",
            "action": "bootsel_reset",
            "uid": info.uid,
            "port": info.port,
        }))?;
    } else {
        println!(
            "BOOTSEL mode requested for {}. Device will re-enumerate as USB Mass Storage.",
            label(&info)
        );
    }
    Ok(())
}

fn label(info: &DeviceInfo) -> String {
    if info.uid.is_empty() {
        info.port.clone()
    } else {
        format!("{} ({})", info.uid, info.port)
    }
}

fn confirm(prompt: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("{prompt} — refusing without --yes when stdin is not a terminal");
    }
    print!("{prompt}? [y/N]: ");
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let answer = buf.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        bail!("aborted by user");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(uid: &str) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_string(),
            device_number: None,
            port: "/dev/ttyACM0".to_string(),
            version: None,
        }
    }

    #[test]
    fn label_falls_back_to_port_without_uid() {
        assert_eq!(label(&info("")), "/dev/ttyACM0");
    }

    #[test]
    fn label_names_uid_and_port() {
        assert_eq!(label(&info("AABB")), "AABB (/dev/ttyACM0)");
    }
}
