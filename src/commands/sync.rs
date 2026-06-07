use anyhow::{Context, Result};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::transport;

pub fn run_arm(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync arm")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync armed (slave mode).");
    }
    Ok(())
}

pub fn run_master(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync master")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync master armed.");
    }
    Ok(())
}

pub fn run_start(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync start")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync started (clock running).");
    }
    Ok(())
}

pub fn run_stop(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync stop")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync stopped.");
    }
    Ok(())
}

pub fn run_count(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync count")?;
    let data = resp.ok_data()?;
    let count = parse_sync_count(data)?;

    if json {
        output::print_json(&serde_json::json!({"clock_count": count}))?;
    } else {
        println!("Clock count: {count} µs");
    }
    Ok(())
}

pub fn run_disarm(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let resp = transport::send_command(&mut port, "sync disarm")?;
    resp.ok_data()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync disarmed.");
    }
    Ok(())
}

pub fn run_sync_session(master_selector: &DeviceSelector, json: bool) -> Result<()> {
    let devices = device::enumerate_devices()?;
    if devices.len() < 2 {
        anyhow::bail!("sync run requires at least 2 devices");
    }

    let master = device::select_device(&devices, master_selector)?;
    let slaves: Vec<_> = devices.iter().filter(|d| d.port != master.port).collect();

    for slave in &slaves {
        let mut port = transport::open_port(&slave.port)?;
        let resp = transport::send_command(&mut port, "sync arm")?;
        resp.ok_data()?;
    }

    let mut master_port = transport::open_port(&master.port)?;
    let resp = transport::send_command(&mut master_port, "sync master")?;
    resp.ok_data()?;

    let resp = transport::send_command(&mut master_port, "sync start")?;
    resp.ok_data()?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut results = Vec::new();

    let resp = transport::send_command(&mut master_port, "sync count")?;
    let master_count = parse_sync_count(resp.ok_data()?)?;
    results.push(serde_json::json!({
        "uid": master.uid,
        "port": master.port,
        "device_number": master.device_number,
        "role": "master",
        "clock_count": master_count,
    }));

    for slave in &slaves {
        let mut port = transport::open_port(&slave.port)?;
        let resp = transport::send_command(&mut port, "sync count")?;
        let count = parse_sync_count(resp.ok_data()?)?;
        results.push(serde_json::json!({
            "uid": slave.uid,
            "port": slave.port,
            "device_number": slave.device_number,
            "role": "slave",
            "clock_count": count,
        }));
    }

    let resp = transport::send_command(&mut master_port, "sync stop")?;
    resp.ok_data()?;

    for slave in &slaves {
        let mut port = transport::open_port(&slave.port)?;
        let resp = transport::send_command(&mut port, "sync disarm")?;
        resp.ok_data()?;
    }

    if json {
        output::print_json(&serde_json::json!({"results": results}))?;
    } else {
        println!("Sync session results:");
        for r in &results {
            println!(
                "  {} ({}) [{}]: clock_count = {}",
                r["uid"].as_str().unwrap_or(""),
                r["port"].as_str().unwrap_or(""),
                r["role"].as_str().unwrap_or(""),
                r["clock_count"]
            );
        }
    }

    Ok(())
}

fn parse_sync_count(data: &str) -> Result<u64> {
    for part in data.split_whitespace() {
        if let Ok(n) = part.parse::<u64>() {
            return Ok(n);
        }
    }
    data.trim().parse::<u64>().context("invalid sync count")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sync_count_basic() {
        assert_eq!(parse_sync_count("12345678").unwrap(), 12345678);
    }

    #[test]
    fn parse_sync_count_with_whitespace() {
        assert_eq!(parse_sync_count("  12345678  ").unwrap(), 12345678);
    }
}
