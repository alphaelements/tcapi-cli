use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tcapi_sdk::reconcile::{self, ReconcileGroup, SourceEvent};
use tcapi_sdk::{Connection, TimestampEvent, TimestampResult, TS_GET_MAX};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::{parse_edge_mode, parse_u32};

/// Time `ts capture` waits between arming and reading the ring.
const CAPTURE_WAIT: Duration = Duration::from_secs(1);

/// Source label used for events of a single-device capture.
const LOCAL_DEVICE: &str = "local";

/// One captured edge as printed by `ts get` / `ts capture`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EventOut {
    pub inputs: u32,
    pub raw_count: u32,
    pub raw_phase: u8,
    pub sync_active: bool,
    /// Host-computed from `raw_count` / `raw_phase` (uncorrected, R-TS1).
    pub timestamp_ns: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TimestampOut {
    pub count: usize,
    /// The device ring overflowed: events were dropped.
    pub overflow: bool,
    pub events: Vec<EventOut>,
}

/// Convert an SDK result for printing; `ts_res_ns` is the device's phase step
/// (`caps.ts_res_ns`).
pub fn to_output(result: &TimestampResult, ts_res_ns: u32) -> TimestampOut {
    TimestampOut {
        count: result.events.len(),
        overflow: result.overflow,
        events: result
            .events
            .iter()
            .map(|ev| EventOut {
                inputs: ev.inputs,
                raw_count: ev.raw_count,
                raw_phase: ev.raw_phase,
                sync_active: ev.sync_active,
                timestamp_ns: ev.ns(ts_res_ns),
            })
            .collect(),
    }
}

pub fn run_arm(selector: &DeviceSelector, mask: &str, mode: &str, json: bool) -> Result<()> {
    let mask_value = parse_u32(mask)?;
    let edge = parse_edge_mode(mode)?;
    let (_info, mut conn) = device::open_device(selector)?;
    conn.ts_arm(mask_value, edge)?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Timestamp armed (mask={mask_value:#x}, mode={mode}).");
    }
    Ok(())
}

pub fn run_status(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    let status = conn.ts_status()?;

    if json {
        output::print_json(&status)?;
    } else {
        println!("Armed:    {}", if status.armed { "yes" } else { "no" });
        println!("Pending:  {}", status.pending);
        println!("Overflow: {}", if status.overflow { "yes" } else { "no" });
    }
    Ok(())
}

pub fn run_get(
    selector: &DeviceSelector,
    max_events: u16,
    do_reconcile: bool,
    json: bool,
) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    get(
        &mut conn,
        max_events,
        do_reconcile,
        json,
        &mut io::stdout().lock(),
    )
}

/// `ts get`: one call of at most [`TS_GET_MAX`] events (the SDK rejects
/// other values locally).
pub fn get(
    conn: &mut Connection,
    max_events: u16,
    do_reconcile: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let ts_res_ns = conn.caps()?.ts_res_ns;
    let result = conn.ts_get(max_events)?;
    print_result(&result, ts_res_ns, do_reconcile, json, out)
}

pub fn run_disarm(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.ts_disarm()?;

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
    let mask_value = parse_u32(mask)?;
    let edge = parse_edge_mode(mode)?;
    let (_info, mut conn) = device::open_device(selector)?;
    let ts_res_ns = conn.caps()?.ts_res_ns;

    conn.ts_arm(mask_value, edge)?;
    if !json {
        println!("Armed. Waiting for events...");
    }
    std::thread::sleep(CAPTURE_WAIT);

    let drained = drain_connection(&mut conn, max_events);
    // Disarm even when reading failed; a disarm failure must not hide the
    // read error that is already being returned.
    let disarmed = conn.ts_disarm();
    let result = drained?;
    disarmed?;

    print_result(&result, ts_res_ns, false, json, &mut io::stdout().lock())
}

fn print_result(
    result: &TimestampResult,
    ts_res_ns: u32,
    do_reconcile: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    if result.overflow {
        eprintln!("Warning: the device ring overflowed; events were dropped.");
    }
    if do_reconcile {
        return print_reconciled(result, ts_res_ns, json, out);
    }
    let events = to_output(result, ts_res_ns);
    if json {
        output::write_json(out, &events)?;
    } else {
        writeln!(out, "Timestamp events: {}", events.count)?;
        for (i, ev) in events.events.iter().enumerate() {
            writeln!(out, "  [{i}] {}", format_event(ev))?;
        }
    }
    Ok(())
}

/// Read up to `max_events` events with as many `ts get` calls as needed (one
/// response carries at most [`TS_GET_MAX`] events). Stops early once the
/// device ring is empty; `overflow` is set if any call reported it.
pub fn drain_events(
    max_events: u16,
    mut get: impl FnMut(u16) -> tcapi_sdk::Result<TimestampResult>,
) -> tcapi_sdk::Result<TimestampResult> {
    let mut drained = TimestampResult {
        events: Vec::new(),
        reconciled: false,
        overflow: false,
    };
    while drained.events.len() < usize::from(max_events) {
        let remaining = usize::from(max_events) - drained.events.len();
        // `remaining` is below `max_events` (u16) and capped to TS_GET_MAX.
        let request =
            u16::try_from(remaining.min(usize::from(TS_GET_MAX))).expect("bounded by TS_GET_MAX");
        let batch = get(request)?;
        let received = batch.events.len();
        drained.overflow |= batch.overflow;
        drained.events.extend(batch.events);
        if received < usize::from(request) {
            break;
        }
    }
    Ok(drained)
}

/// Read every event of `conn`'s ring up to `max_events`.
pub fn drain_connection(
    conn: &mut Connection,
    max_events: u16,
) -> tcapi_sdk::Result<TimestampResult> {
    drain_events(max_events, |max| conn.ts_get(max))
}

fn format_event(ev: &EventOut) -> String {
    format!(
        "inputs={:#010x} {}ns (c={} p={} s={})",
        ev.inputs,
        ev.timestamp_ns,
        ev.raw_count,
        ev.raw_phase,
        u8::from(ev.sync_active)
    )
}

fn print_reconciled(
    result: &TimestampResult,
    ts_res_ns: u32,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let source_events = source_events(LOCAL_DEVICE, &result.events);
    let groups = reconcile::group_events(&source_events, ts_res_ns);

    if json {
        let mut value = groups_to_json(&groups);
        value["overflow"] = serde_json::json!(result.overflow);
        output::write_json(out, &value)?;
    } else {
        let reconciled_groups = groups.iter().filter(|g| g.reconciled).count();
        writeln!(
            out,
            "Timestamp events: {} ({} groups, {} reconciled)",
            result.events.len(),
            groups.len(),
            reconciled_groups
        )?;
        write_groups_human(out, &groups)?;
    }
    Ok(())
}

/// Tag every event of one device with its source and channel label for
/// [`reconcile::group_events`].
pub fn source_events(device: &str, events: &[TimestampEvent]) -> Vec<SourceEvent> {
    events
        .iter()
        .zip(assign_channels(events))
        .map(|(ev, channel)| SourceEvent {
            device: device.to_string(),
            channel,
            inputs: format!("{:#010x}", ev.inputs),
            raw_count: ev.raw_count,
            raw_phase: ev.raw_phase,
            sync_active: ev.sync_active,
        })
        .collect()
}

pub fn write_groups_human(out: &mut impl Write, groups: &[ReconcileGroup]) -> Result<()> {
    for (gi, group) in groups.iter().enumerate() {
        let tag = if group.reconciled {
            ""
        } else {
            " [uncorrected]"
        };
        writeln!(out, "  Group {gi} (count≈{}){tag}:", group.approx_count)?;
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
        writeln!(out, "{line}")?;
        if let Some(w) = &group.warning {
            writeln!(out, "    ⚠ {w}")?;
        }
    }
    Ok(())
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

pub fn groups_to_json(groups: &[ReconcileGroup]) -> serde_json::Value {
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
            let changed = prev.map_or(ev.inputs, |p| ev.inputs ^ p);
            prev = Some(ev.inputs);
            bits_to_label(changed)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(inputs: u32, count: u32, phase: u8) -> TimestampEvent {
        TimestampEvent {
            inputs,
            raw_count: count,
            raw_phase: phase,
            sync_active: true,
        }
    }

    fn result_of(events: Vec<TimestampEvent>, overflow: bool) -> TimestampResult {
        TimestampResult {
            events,
            reconciled: false,
            overflow,
        }
    }

    #[test]
    fn output_computes_ns_from_device_resolution() {
        let result = result_of(
            vec![
                make_event(0x5, 1000, 7),
                TimestampEvent {
                    sync_active: false,
                    ..make_event(0xa, 2000, 12)
                },
            ],
            true,
        );
        let out = to_output(&result, 40);
        assert_eq!(out.count, 2);
        assert!(out.overflow);
        // sync active: count * 1000 + phase * ts_res_ns
        assert_eq!(out.events[0].timestamp_ns, 1_000_280);
        // free running: phase is ignored
        assert_eq!(out.events[1].timestamp_ns, 2_000_000);
        assert_eq!(out.events[0].inputs, 5);
    }

    #[test]
    fn output_json_keeps_all_fields() {
        let out = to_output(&result_of(vec![make_event(1, 10, 2)], false), 40);
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["count"], 1);
        assert_eq!(json["overflow"], false);
        assert_eq!(json["events"][0]["timestamp_ns"], 10_080);
        assert_eq!(json["events"][0]["sync_active"], true);
    }

    #[test]
    fn event_line_shows_ns_and_raw_fields() {
        let out = to_output(&result_of(vec![make_event(5, 1000, 7)], false), 40);
        assert_eq!(
            format_event(&out.events[0]),
            "inputs=0x00000005 1000280ns (c=1000 p=7 s=1)"
        );
    }

    #[test]
    fn channels_come_from_input_diffs() {
        let events = [
            make_event(0b01, 1000, 5), // GP0 rises
            make_event(0b11, 1000, 9), // GP1 rises
            make_event(0b10, 2000, 1), // GP0 falls
        ];
        assert_eq!(assign_channels(&events), ["GP0", "GP1", "GP0"]);
    }

    #[test]
    fn simultaneous_changes_get_a_compound_label() {
        let events = [make_event(0b001, 1000, 5), make_event(0b111, 1000, 9)];
        assert_eq!(assign_channels(&events), ["GP0", "GP1+GP2"]);
    }

    #[test]
    fn unchanged_snapshot_is_labelled_unknown() {
        let events = [make_event(0b1, 1, 0), make_event(0b1, 2, 0)];
        assert_eq!(assign_channels(&events), ["GP0", "GP?"]);
    }

    #[test]
    fn source_events_carry_device_and_hex_inputs() {
        let sources = source_events("local", &[make_event(0x5, 1000, 7)]);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].device, "local");
        assert_eq!(sources[0].channel, "GP0+GP2");
        assert_eq!(sources[0].inputs, "0x00000005");
        assert_eq!(sources[0].raw_count, 1000);
    }

    #[test]
    fn groups_json_disambiguates_colliding_labels() {
        let events = [make_event(0b1, 1000, 5), make_event(0b1, 1000, 6)];
        // Both events end up labelled GP0 / GP? within one group.
        let sources = vec![
            SourceEvent {
                channel: "GP0".into(),
                ..source_events("local", &events[..1])[0].clone()
            },
            SourceEvent {
                channel: "GP0".into(),
                ..source_events("local", &events[1..])[0].clone()
            },
        ];
        let groups = reconcile::group_events(&sources, 40);
        let json = groups_to_json(&groups);
        let keys: Vec<&String> = json["groups"][0]["events"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|k| k.ends_with("#2")));
    }

    #[test]
    fn drain_collects_across_several_calls() {
        let mut requests = Vec::new();
        let mut remaining = 200usize;
        let drained = drain_events(256, |max| {
            requests.push(max);
            let take = remaining.min(usize::from(max));
            remaining -= take;
            Ok(result_of(vec![make_event(1, 1, 0); take], false))
        })
        .unwrap();
        assert_eq!(drained.events.len(), 200);
        assert_eq!(requests, [85, 85, 85]);
        assert!(!drained.overflow);
    }

    #[test]
    fn drain_stops_at_the_requested_maximum() {
        let mut requests = Vec::new();
        let drained = drain_events(100, |max| {
            requests.push(max);
            Ok(result_of(
                vec![make_event(1, 1, 0); usize::from(max)],
                false,
            ))
        })
        .unwrap();
        assert_eq!(drained.events.len(), 100);
        assert_eq!(requests, [85, 15]);
    }

    #[test]
    fn drain_of_empty_ring_is_one_call() {
        let mut calls = 0;
        let drained = drain_events(256, |_| {
            calls += 1;
            Ok(result_of(vec![], false))
        })
        .unwrap();
        assert!(drained.events.is_empty());
        assert_eq!(calls, 1);
    }

    #[test]
    fn drain_remembers_overflow_from_any_call() {
        let mut first = true;
        let drained = drain_events(170, |max| {
            let overflow = std::mem::take(&mut first);
            Ok(result_of(
                vec![make_event(1, 1, 0); usize::from(max)],
                overflow,
            ))
        })
        .unwrap();
        assert!(drained.overflow);
        assert_eq!(drained.events.len(), 170);
    }

    #[test]
    fn drain_propagates_device_errors() {
        let err = drain_events(10, |_| Err(tcapi_sdk::SdkError::NotArmed)).unwrap_err();
        assert!(matches!(err, tcapi_sdk::SdkError::NotArmed));
    }

    #[test]
    fn drain_with_zero_maximum_reads_nothing() {
        let drained = drain_events(0, |_| panic!("must not be called")).unwrap();
        assert!(drained.events.is_empty());
    }
}
