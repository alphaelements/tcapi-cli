use anyhow::{Context, Result};
use serde::Serialize;

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::reconcile::{self, SourceEvent};
use crate::transport;

#[derive(Debug, Serialize)]
pub struct TimestampEvent {
    pub inputs: String,
    pub raw_count: u32,
    pub raw_phase: u8,
    pub sync_active: bool,
    /// Host-computed: raw_count * 1000 + raw_phase * 40 (uncorrected).
    pub timestamp_ns: u64,
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

pub fn run_get(
    selector: &DeviceSelector,
    max_events: u16,
    do_reconcile: bool,
    json: bool,
) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let cmd = format!("ts get {max_events}");
    let lines = transport::send_command_counted(&mut port, &cmd)?;
    let parsed = parse_ts_response(&lines)?;

    if do_reconcile {
        print_reconciled(&parsed, json)
    } else if json {
        output::print_json(&parsed)?;
        Ok(())
    } else {
        print_events(&parsed);
        Ok(())
    }
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
    let lines = transport::send_command_counted(&mut port, &get_cmd)?;
    let parsed = parse_ts_response(&lines)?;

    let _ = transport::send_command(&mut port, "ts disarm");

    if json {
        output::print_json(&parsed)?;
    } else {
        println!("Captured {} events:", parsed.count);
        for (i, ev) in parsed.events.iter().enumerate() {
            println!("  [{i}] {}", format_event(ev));
        }
    }
    Ok(())
}

fn print_events(parsed: &TimestampResult) {
    println!("Timestamp events: {}", parsed.count);
    for (i, ev) in parsed.events.iter().enumerate() {
        println!("  [{i}] {}", format_event(ev));
    }
}

fn format_event(ev: &TimestampEvent) -> String {
    format!(
        "inputs={} {}ns (c={} p={} s={})",
        ev.inputs,
        ev.timestamp_ns,
        ev.raw_count,
        ev.raw_phase,
        if ev.sync_active { 1 } else { 0 }
    )
}

fn print_reconciled(parsed: &TimestampResult, json: bool) -> Result<()> {
    let channels = assign_channels(&parsed.events);
    let source_events: Vec<SourceEvent> = parsed
        .events
        .iter()
        .zip(channels.iter())
        .map(|(ev, ch)| SourceEvent {
            device: "local".to_string(),
            channel: ch.clone(),
            inputs: ev.inputs.clone(),
            raw_count: ev.raw_count,
            raw_phase: ev.raw_phase,
            sync_active: ev.sync_active,
        })
        .collect();

    let groups = reconcile::group_events(&source_events);

    if json {
        output::print_json(&groups_to_json(&groups))?;
    } else {
        let reconciled_groups = groups.iter().filter(|g| g.reconciled).count();
        println!(
            "Timestamp events: {} ({} groups, {} reconciled)",
            parsed.count,
            groups.len(),
            reconciled_groups
        );
        print_groups_human(&groups);
    }
    Ok(())
}

pub fn print_groups_human(groups: &[reconcile::ReconcileGroup]) {
    for (gi, group) in groups.iter().enumerate() {
        let tag = if group.reconciled {
            ""
        } else {
            " [uncorrected]"
        };
        println!("  Group {gi} (count≈{}){tag}:", group.approx_count);
        let parts: Vec<String> = group
            .events
            .iter()
            .zip(group.reconciled_ns.iter())
            .map(|(ev, ns)| format!("{}: {}ns", ev.source_label(), ns))
            .collect();
        let mut line = format!("    {}", parts.join("  "));
        if group.events.len() == 2 {
            let diff = group.reconciled_ns[0] - group.reconciled_ns[1];
            line.push_str(&format!("  diff={diff:+}ns"));
        } else if group.events.len() > 2 {
            let max_diff = max_pairwise_abs_diff(&group.reconciled_ns);
            line.push_str(&format!("  max|diff|={max_diff}ns"));
        }
        println!("{line}");
        if let Some(w) = &group.warning {
            println!("    ⚠ {w}");
        }
    }
}

fn max_pairwise_abs_diff(values: &[i64]) -> i64 {
    let mut max = 0;
    for i in 0..values.len() {
        for j in (i + 1)..values.len() {
            max = max.max((values[i] - values[j]).abs());
        }
    }
    max
}

pub fn groups_to_json(groups: &[reconcile::ReconcileGroup]) -> serde_json::Value {
    let groups_json: Vec<serde_json::Value> = groups
        .iter()
        .enumerate()
        .map(|(gi, group)| {
            let mut events = serde_json::Map::new();
            for (ev, ns) in group.events.iter().zip(group.reconciled_ns.iter()) {
                // Channel attribution is heuristic, so labels can collide
                // within a group; disambiguate instead of overwriting.
                let mut key = ev.source_label();
                let mut suffix = 2;
                while events.contains_key(&key) {
                    key = format!("{}#{suffix}", ev.source_label());
                    suffix += 1;
                }
                events.insert(
                    key,
                    serde_json::json!({
                        "inputs": ev.inputs,
                        "raw_count": ev.raw_count,
                        "raw_phase": ev.raw_phase,
                        "sync_active": ev.sync_active,
                        "reconciled_ns": ns,
                    }),
                );
            }
            serde_json::json!({
                "index": gi,
                "approx_count": group.approx_count,
                "reconciled": group.reconciled,
                "warning": group.warning,
                "events": events,
            })
        })
        .collect();
    serde_json::json!({ "groups": groups_json })
}

/// Best-effort channel attribution: the edge channel(s) of each event are
/// the bits that changed vs the previous snapshot (first event: bits set
/// in its own snapshot). The device only reports full input snapshots, so
/// this is heuristic — simultaneous multi-bit changes get a compound label.
pub fn assign_channels(events: &[TimestampEvent]) -> Vec<String> {
    let mut prev: Option<u32> = None;
    events
        .iter()
        .map(|ev| {
            let cur = parse_hex_u32(&ev.inputs).unwrap_or(0);
            let changed = match prev {
                Some(p) => cur ^ p,
                None => cur,
            };
            prev = Some(cur);
            bits_to_label(changed)
        })
        .collect()
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    u32::from_str_radix(hex, 16).ok()
}

fn bits_to_label(bits: u32) -> String {
    if bits == 0 {
        return "GP?".to_string();
    }
    let names: Vec<String> = (0..32)
        .filter(|b| bits & (1 << b) != 0)
        .map(|b| format!("GP{b}"))
        .collect();
    names.join("+")
}

/// Parse a `ts get` response (FW Phase 9.6 format):
/// ```text
/// OK count=2
///   0x00000005 c=1000 p=7 s=1
///   0x0000000A c=2000 p=12 s=0
/// ```
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
            let mut inputs = String::new();
            let mut raw_count: Option<u32> = None;
            let mut raw_phase: Option<u8> = None;
            let mut sync_active: Option<bool> = None;

            for part in trimmed.split_whitespace() {
                if part.starts_with("0x") || part.starts_with("0X") {
                    inputs = part.to_string();
                } else if let Some(val) = part.strip_prefix("c=") {
                    raw_count = Some(val.parse().context("invalid raw_count")?);
                } else if let Some(val) = part.strip_prefix("p=") {
                    raw_phase = Some(val.parse().context("invalid raw_phase")?);
                } else if let Some(val) = part.strip_prefix("s=") {
                    sync_active = Some(match val {
                        "0" => false,
                        "1" => true,
                        _ => anyhow::bail!("invalid sync_active: {val}"),
                    });
                }
            }

            let raw_count = raw_count.context("missing c= field in ts event")?;
            let raw_phase = raw_phase.context("missing p= field in ts event")?;
            let sync_active = sync_active.context("missing s= field in ts event")?;
            let timestamp_ns = raw_count as u64 * 1000 + raw_phase as u64 * 40;

            events.push(TimestampEvent {
                inputs,
                raw_count,
                raw_phase,
                sync_active,
                timestamp_ns,
            });
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
    fn parse_ts_response_new_format() {
        let lines = vec![
            "OK count=2".to_string(),
            "  0x00000005 c=1000 p=7 s=1".to_string(),
            "  0x0000000A c=2000 p=12 s=0".to_string(),
        ];
        let r = parse_ts_response(&lines).unwrap();
        assert_eq!(r.count, 2);
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.events[0].inputs, "0x00000005");
        assert_eq!(r.events[0].raw_count, 1000);
        assert_eq!(r.events[0].raw_phase, 7);
        assert!(r.events[0].sync_active);
        assert_eq!(r.events[0].timestamp_ns, 1000280);
        assert_eq!(r.events[1].raw_count, 2000);
        assert_eq!(r.events[1].raw_phase, 12);
        assert!(!r.events[1].sync_active);
        assert_eq!(r.events[1].timestamp_ns, 2000480);
    }

    #[test]
    fn parse_ts_response_sync_active() {
        let lines = vec![
            "OK count=1".to_string(),
            "  0x00000001 c=500 p=0 s=0".to_string(),
        ];
        let r = parse_ts_response(&lines).unwrap();
        assert!(!r.events[0].sync_active);
        assert_eq!(r.events[0].timestamp_ns, 500000);
    }

    #[test]
    fn parse_ts_response_mixed_sync() {
        let lines = vec![
            "OK count=2".to_string(),
            "  0x00000001 c=500 p=3 s=1".to_string(),
            "  0x00000002 c=600 p=0 s=0".to_string(),
        ];
        let r = parse_ts_response(&lines).unwrap();
        assert!(r.events[0].sync_active);
        assert!(!r.events[1].sync_active);
    }

    #[test]
    fn parse_ts_response_empty() {
        let lines = vec!["OK count=0".to_string()];
        let r = parse_ts_response(&lines).unwrap();
        assert_eq!(r.count, 0);
        assert!(r.events.is_empty());
    }

    #[test]
    fn parse_ts_response_old_format_rejected() {
        // Old `t=N` format (≤ Phase 9.5) must not silently parse.
        let lines = vec!["OK count=1".to_string(), "  0x00000005 t=1000".to_string()];
        assert!(parse_ts_response(&lines).is_err());
    }

    fn make_event(inputs: &str, count: u32, phase: u8) -> TimestampEvent {
        TimestampEvent {
            inputs: inputs.to_string(),
            raw_count: count,
            raw_phase: phase,
            sync_active: true,
            timestamp_ns: count as u64 * 1000 + phase as u64 * 40,
        }
    }

    #[test]
    fn assign_channels_from_diffs() {
        let events = vec![
            make_event("0x00000001", 1000, 5), // GP0 rises
            make_event("0x00000003", 1000, 9), // GP1 rises
            make_event("0x00000002", 2000, 1), // GP0 falls
        ];
        let chs = assign_channels(&events);
        assert_eq!(chs, vec!["GP0", "GP1", "GP0"]);
    }

    #[test]
    fn assign_channels_simultaneous() {
        let events = vec![
            make_event("0x00000001", 1000, 5),
            make_event("0x00000007", 1000, 9), // GP1+GP2 rise together
        ];
        let chs = assign_channels(&events);
        assert_eq!(chs, vec!["GP0", "GP1+GP2"]);
    }
}
