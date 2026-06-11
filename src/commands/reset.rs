use std::io::{IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Result};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

const RESET_LATENCY: Duration = Duration::from_millis(50);

pub fn run_system_reset(selector: &DeviceSelector, yes: bool, json: bool) -> Result<()> {
    if !yes {
        confirm("Soft-reset the device? USB will disconnect and re-enumerate")?;
    }
    let (info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "reset")?;
    resp.ok_data()?;

    // FW waits ~50ms after OK before issuing sys_reset. Drop the port and
    // sleep a moment so the host sees a clean disconnect on the next list.
    drop(port);
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
    let (info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "bootsel")?;
    resp.ok_data()?;

    drop(port);
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

fn label(info: &device::DeviceInfo) -> String {
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
