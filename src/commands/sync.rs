use anyhow::{Context, Result};

use crate::commands::timestamp::{self, TimestampEvent};
use crate::device::{self, DeviceInfo, DeviceSelector};
use crate::output;
use crate::reconcile::{self, CountExtender, SourceEvent};
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
        println!(
            "Clock count: {count} µs ({:.3} s)",
            count as f64 / 1_000_000.0
        );
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
        "diff_us": 0,
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
            "diff_us": count as i64 - master_count as i64,
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
            let role = r["role"].as_str().unwrap_or("");
            let mut line = format!(
                "  {} ({}) [{}]: clock_count = {} µs",
                r["uid"].as_str().unwrap_or(""),
                r["port"].as_str().unwrap_or(""),
                role,
                r["clock_count"]
            );
            if role == "slave" {
                let diff = r["diff_us"].as_i64().unwrap_or(0);
                line.push_str(&format!("  (diff: {diff:+} µs)"));
            }
            println!("{line}");
        }
    }

    Ok(())
}

/// `pochi sync capture` — multi-device synchronized timestamp capture with
/// host-side reconciliation:
/// 1. enumerate (2+ devices), pick master (selector or first device)
/// 2. slaves: sync arm; master: sync master → sync start
/// 3. all: ts arm <mask> <mode>
/// 4. wait --duration seconds (Ctrl+C to stop early)
/// 5. all: ts get; group by count proximity, reconcile, show pair stats
/// 6. all: ts disarm; master: sync stop; slaves: sync disarm
#[allow(clippy::too_many_arguments)]
pub fn run_sync_capture(
    master_selector: &DeviceSelector,
    mask: &str,
    mode: &str,
    max_events: u16,
    duration_secs: u64,
    json: bool,
) -> Result<()> {
    let devices = device::enumerate_devices()?;
    if devices.len() < 2 {
        anyhow::bail!("sync capture requires at least 2 devices");
    }

    let has_selector = master_selector.uid.is_some()
        || master_selector.device.is_some()
        || master_selector.port.is_some();
    let master = if has_selector {
        device::select_device(&devices, master_selector)?
    } else {
        devices[0].clone()
    };
    let slaves: Vec<DeviceInfo> = devices
        .iter()
        .filter(|d| d.port != master.port)
        .cloned()
        .collect();

    if !json {
        println!(
            "Sync capture: master={} slaves={}",
            master.uid,
            slaves
                .iter()
                .map(|s| s.uid.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // -- sync session up --
    for slave in &slaves {
        let mut port = transport::open_port(&slave.port)?;
        transport::send_command(&mut port, "sync arm")?.ok_data()?;
    }
    let mut master_port = transport::open_port(&master.port)?;
    transport::send_command(&mut master_port, "sync master")?.ok_data()?;
    transport::send_command(&mut master_port, "sync start")?.ok_data()?;

    // Let the clocking settle before arming timestamps.
    std::thread::sleep(std::time::Duration::from_millis(100));

    let capture_result = (|| -> Result<Vec<(DeviceInfo, Vec<TimestampEvent>)>> {
        for dev in std::iter::once(&master).chain(slaves.iter()) {
            let mut port = transport::open_port(&dev.port)?;
            transport::send_command(&mut port, &format!("ts arm {mask} {mode}"))?
                .ok_data()
                .with_context(|| format!("ts arm failed on {}", dev.uid))?;
        }

        if !json {
            println!("Capturing for {duration_secs}s (Ctrl+C to stop early)...");
        }
        wait_with_ctrlc(duration_secs);

        let mut per_device = Vec::new();
        for dev in std::iter::once(&master).chain(slaves.iter()) {
            let mut port = transport::open_port(&dev.port)?;
            let lines =
                transport::send_command_counted(&mut port, &format!("ts get {max_events}"))?;
            let parsed = timestamp::parse_ts_response(&lines)
                .with_context(|| format!("ts get failed on {}", dev.uid))?;
            let _ = transport::send_command(&mut port, "ts disarm");
            per_device.push((dev.clone(), parsed.events));
        }
        Ok(per_device)
    })();

    // -- sync session down (always, even on capture error) --
    let _ = transport::send_command(&mut master_port, "sync stop");
    let _ = transport::send_command(&mut master_port, "sync disarm");
    for slave in &slaves {
        if let Ok(mut port) = transport::open_port(&slave.port) {
            let _ = transport::send_command(&mut port, "sync disarm");
        }
    }

    let per_device = capture_result?;

    // -- reconcile across all sources (N devices × M channels) --
    let mut source_events: Vec<SourceEvent> = Vec::new();
    for (dev, events) in &per_device {
        // Per-device 32→64bit wrap extension; sequential processing.
        // Grouping/reconcile use the raw u32 count (wrap-window << capture
        // duration), the extender just validates monotonicity here.
        let mut extender = CountExtender::new();
        let channels = timestamp::assign_channels(events);
        for (ev, ch) in events.iter().zip(channels.iter()) {
            extender.extend(ev.raw_count);
            source_events.push(SourceEvent {
                device: dev.uid.clone(),
                channel: ch.clone(),
                inputs: ev.inputs.clone(),
                raw_count: ev.raw_count,
                raw_phase: ev.raw_phase,
                sync_active: ev.sync_active,
            });
        }
    }

    let groups = reconcile::group_events(&source_events);
    let stats = reconcile::pair_stats(&groups);

    if json {
        let mut value = timestamp::groups_to_json(&groups);
        let devices_json: Vec<&str> = per_device.iter().map(|(d, _)| d.uid.as_str()).collect();
        value["devices"] = serde_json::json!(devices_json);
        value["stats"] = serde_json::json!({ "pairs": stats });
        output::print_json(&value)?;
    } else {
        let total_events: usize = per_device.iter().map(|(_, e)| e.len()).sum();
        println!(
            "Sync capture: {} devices, {} events, {} groups",
            per_device.len(),
            total_events,
            groups.len()
        );
        timestamp::print_groups_human(&groups);

        if !stats.is_empty() {
            println!();
            println!("Stats (per source pair):");
            for s in &stats {
                println!(
                    "  {} vs {}: n={} mean={:+.1}ns stdev={:.1}ns max={}ns  {:.1}% ≤500ns",
                    s.a,
                    s.b,
                    s.n,
                    s.mean_diff_ns,
                    s.stdev_ns,
                    s.max_abs_diff_ns,
                    s.within_500ns_pct
                );
            }
        }
    }

    Ok(())
}

fn wait_with_ctrlc(duration_secs: u64) {
    let ctrlc_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ctrlc_flag.clone();
    // Ignore error: handler may already be set in long-running sessions.
    let _ = ctrlc::set_handler(move || {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(duration_secs);
    while std::time::Instant::now() < deadline {
        if ctrlc_flag.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Parse a `sync count` response (`count=12345`, bare numbers accepted).
fn parse_sync_count(data: &str) -> Result<u64> {
    for part in data.split_whitespace() {
        let candidate = part.strip_prefix("count=").unwrap_or(part);
        if let Ok(n) = candidate.parse::<u64>() {
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

    #[test]
    fn parse_sync_count_with_prefix() {
        assert_eq!(parse_sync_count("count=12345").unwrap(), 12345);
    }
}
