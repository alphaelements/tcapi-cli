use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use pochi_sdk::reconcile::{self, PairStats, ReconcileGroup};
use pochi_sdk::{Connection, DeviceInfo, EdgeMode, TimestampEvent};
use serde::Serialize;

use crate::commands::timestamp;
use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::{parse_edge_mode, parse_u32};

/// Time the master's clock needs to settle before counts or timestamps are
/// meaningful.
const CLOCK_SETTLE: Duration = Duration::from_millis(100);

/// How often the capture wait checks the Ctrl+C flag.
const CAPTURE_POLL: Duration = Duration::from_millis(100);

pub fn run_arm(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.sync_arm()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync armed (slave mode).");
    }
    Ok(())
}

pub fn run_master(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.sync_master()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync master armed.");
    }
    Ok(())
}

pub fn run_start(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.sync_start()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync started (clock running).");
    }
    Ok(())
}

pub fn run_stop(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.sync_stop()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync stopped.");
    }
    Ok(())
}

pub fn run_count(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    let count = conn.sync_count()?;

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
    let (_info, mut conn) = device::open_device(selector)?;
    conn.sync_disarm()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Sync disarmed.");
    }
    Ok(())
}

/// One device taking part in a multi-device sync operation.
struct Member {
    info: DeviceInfo,
    conn: Connection,
}

/// The enumerated devices split into the sync master and its slaves, each
/// with an open connection.
struct Group {
    master: Member,
    slaves: Vec<Member>,
}

impl Group {
    /// Enumerate (at least two devices), choose the master (`--port`, `--uid`
    /// or `--device`, else the first device) and open every device.
    fn open(master_selector: &DeviceSelector, what: &str) -> Result<Self> {
        let devices = device::enumerate_devices()?;
        if devices.len() < 2 {
            bail!("{what} requires at least 2 devices");
        }
        let has_selector = master_selector.uid.is_some()
            || master_selector.device.is_some()
            || master_selector.port.is_some();
        let master = if has_selector {
            device::select_device(&devices, master_selector)?
        } else {
            devices[0].clone()
        };

        let open = |info: DeviceInfo| -> Result<Member> {
            let conn = device::open_port(&info.port)?;
            Ok(Member { info, conn })
        };
        let slaves = devices
            .iter()
            .filter(|d| d.port != master.port)
            .cloned()
            .map(open)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            master: open(master)?,
            slaves,
        })
    }

    /// Slaves `sync arm`, then the master `sync master` and `sync start`.
    fn start_sync(&mut self) -> Result<()> {
        for slave in &mut self.slaves {
            slave.conn.sync_arm()?;
        }
        self.master.conn.sync_master()?;
        self.master.conn.sync_start()?;
        // Let the clocking settle before reading counts or arming timestamps.
        std::thread::sleep(CLOCK_SETTLE);
        Ok(())
    }

    /// Stop the clock and release the sync state of every device.
    ///
    /// This runs after the measurement, also when it failed. A failure here
    /// is reported as a warning rather than returned: the measurement result
    /// (or its own error) is what the caller needs, and every remaining
    /// device must still be released.
    fn stop_sync(&mut self) {
        let warn = |what: &str, uid: &str, result: pochi_sdk::Result<()>| {
            if let Err(e) = result {
                eprintln!("warning: {what} failed on {uid}: {e}");
            }
        };
        warn(
            "sync stop",
            &self.master.info.uid,
            self.master.conn.sync_stop(),
        );
        warn(
            "sync disarm",
            &self.master.info.uid,
            self.master.conn.sync_disarm(),
        );
        for slave in &mut self.slaves {
            warn("sync disarm", &slave.info.uid, slave.conn.sync_disarm());
        }
    }

    fn members_mut(&mut self) -> impl Iterator<Item = &mut Member> {
        std::iter::once(&mut self.master).chain(self.slaves.iter_mut())
    }
}

/// The events one device captured.
type DeviceEvents = (DeviceInfo, Vec<TimestampEvent>);

/// One line of the `sync run` report.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SessionRow {
    pub uid: String,
    pub port: String,
    pub device_number: Option<u8>,
    pub role: &'static str,
    pub clock_count: u64,
    /// Slave clock count minus master clock count (0 for the master).
    pub diff_us: i64,
}

/// Build the report rows: the master first, then the slaves in order.
pub fn session_rows(master: (&DeviceInfo, u64), slaves: &[(&DeviceInfo, u64)]) -> Vec<SessionRow> {
    let row = |info: &DeviceInfo, role: &'static str, count: u64, diff_us: i64| SessionRow {
        uid: info.uid.clone(),
        port: info.port.clone(),
        device_number: info.device_number,
        role,
        clock_count: count,
        diff_us,
    };
    let (master_info, master_count) = master;
    let mut rows = vec![row(master_info, "master", master_count, 0)];
    rows.extend(slaves.iter().map(|(info, count)| {
        // Counts are microseconds since sync start: far below i64::MAX.
        row(info, "slave", *count, *count as i64 - master_count as i64)
    }));
    rows
}

fn format_session_row(row: &SessionRow) -> String {
    let mut line = format!(
        "  {} ({}) [{}]: clock_count = {} µs",
        row.uid, row.port, row.role, row.clock_count
    );
    if row.role == "slave" {
        line.push_str(&format!("  (diff: {:+} µs)", row.diff_us));
    }
    line
}

pub fn run_sync_session(master_selector: &DeviceSelector, json: bool) -> Result<()> {
    let mut group = Group::open(master_selector, "sync run")?;
    group.start_sync()?;

    let measured = (|| -> Result<Vec<SessionRow>> {
        let master_count = group.master.conn.sync_count()?;
        let mut slave_counts = Vec::new();
        for slave in &mut group.slaves {
            slave_counts.push(slave.conn.sync_count()?);
        }
        let slaves: Vec<(&DeviceInfo, u64)> = group
            .slaves
            .iter()
            .map(|m| &m.info)
            .zip(slave_counts)
            .collect();
        Ok(session_rows((&group.master.info, master_count), &slaves))
    })();
    group.stop_sync();
    let rows = measured?;

    if json {
        output::print_json(&serde_json::json!({ "results": rows }))?;
    } else {
        println!("Sync session results:");
        for row in &rows {
            println!("{}", format_session_row(row));
        }
    }
    Ok(())
}

/// Reconcile the events of every device as one set of sources.
pub fn reconcile_devices(
    per_device: &[DeviceEvents],
    ts_res_ns: u32,
) -> (Vec<ReconcileGroup>, Vec<PairStats>) {
    let source_events: Vec<_> = per_device
        .iter()
        .flat_map(|(dev, events)| timestamp::source_events(&dev.uid, events))
        .collect();
    let groups = reconcile::group_events(&source_events, ts_res_ns);
    let stats = reconcile::pair_stats(&groups);
    (groups, stats)
}

/// `pochi sync capture` — multi-device synchronized timestamp capture with
/// host-side reconciliation:
/// 1. enumerate (2+ devices), pick master (selector or first device)
/// 2. slaves: sync arm; master: sync master → sync start
/// 3. all: ts arm <mask> <mode>
/// 4. wait --duration seconds (Ctrl+C to stop early)
/// 5. all: ts get; group by count proximity, reconcile, show pair stats
/// 6. all: ts disarm; master: sync stop; slaves: sync disarm
pub fn run_sync_capture(
    master_selector: &DeviceSelector,
    mask: &str,
    mode: &str,
    max_events: u16,
    duration_secs: u64,
    json: bool,
) -> Result<()> {
    let mask = parse_u32(mask)?;
    let edge = parse_edge_mode(mode)?;
    let mut group = Group::open(master_selector, "sync capture")?;

    if !json {
        println!(
            "Sync capture: master={} slaves={}",
            group.master.info.uid,
            group
                .slaves
                .iter()
                .map(|s| s.info.uid.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    group.start_sync()?;
    let captured = capture_all(&mut group, mask, edge, max_events, duration_secs, json);
    // Always release the devices, also when the capture failed.
    release_timestamps(&mut group);
    group.stop_sync();
    let (per_device, ts_res_ns) = captured?;

    let (groups, stats) = reconcile_devices(&per_device, ts_res_ns);

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
        timestamp::write_groups_human(&mut io::stdout().lock(), &groups)?;

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

/// Arm every device, wait, then read every ring. Returns the events per
/// device and the timestamp resolution shared by all devices.
fn capture_all(
    group: &mut Group,
    mask: u32,
    edge: EdgeMode,
    max_events: u16,
    duration_secs: u64,
    json: bool,
) -> Result<(Vec<DeviceEvents>, u32)> {
    let ts_res_ns = shared_resolution(group)?;
    for member in group.members_mut() {
        member.conn.ts_arm(mask, edge)?;
    }

    if !json {
        println!("Capturing for {duration_secs}s (Ctrl+C to stop early)...");
    }
    wait_with_ctrlc(duration_secs);

    let mut per_device = Vec::new();
    for member in group.members_mut() {
        let result = timestamp::drain_connection(&mut member.conn, max_events)?;
        if result.overflow {
            eprintln!(
                "warning: the ring of {} overflowed; events were dropped",
                member.info.uid
            );
        }
        per_device.push((member.info.clone(), result.events));
    }
    Ok((per_device, ts_res_ns))
}

/// The timestamp phase step of the devices. Events of devices with different
/// steps cannot be reconciled together.
fn shared_resolution(group: &mut Group) -> Result<u32> {
    let mut resolution: Option<u32> = None;
    for member in group.members_mut() {
        let device_res = member.conn.caps()?.ts_res_ns;
        match resolution {
            None => resolution = Some(device_res),
            Some(res) if res != device_res => bail!(
                "devices report different timestamp resolutions ({res} ns vs {device_res} ns); \
                 cannot reconcile them together"
            ),
            Some(_) => {}
        }
    }
    resolution.ok_or_else(|| anyhow::anyhow!("no devices"))
}

/// Disarm timestamp capture on every device, warning on failure (see
/// [`Group::stop_sync`] for why this is not an error).
fn release_timestamps(group: &mut Group) {
    for member in group.members_mut() {
        if let Err(e) = member.conn.ts_disarm() {
            eprintln!("warning: ts disarm failed on {}: {e}", member.info.uid);
        }
    }
}

fn wait_with_ctrlc(duration_secs: u64) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    // Installing fails only if a handler is already set (it is installed
    // once per process); the wait then just runs for the full duration.
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed));

    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(CAPTURE_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(uid: &str, port: &str, number: Option<u8>) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_string(),
            port: port.to_string(),
            device_number: number,
            version: None,
        }
    }

    fn event(inputs: u32, count: u32, phase: u8) -> TimestampEvent {
        TimestampEvent {
            inputs,
            raw_count: count,
            raw_phase: phase,
            sync_active: true,
        }
    }

    #[test]
    fn session_rows_start_with_the_master_and_diff_slaves_against_it() {
        let master = info("M", "/dev/m", Some(1));
        let slave_a = info("A", "/dev/a", None);
        let slave_b = info("B", "/dev/b", Some(3));
        let rows = session_rows((&master, 1_000), &[(&slave_a, 1_005), (&slave_b, 990)]);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].role, "master");
        assert_eq!(rows[0].diff_us, 0);
        assert_eq!(rows[0].device_number, Some(1));
        assert_eq!((rows[1].uid.as_str(), rows[1].diff_us), ("A", 5));
        assert_eq!((rows[2].uid.as_str(), rows[2].diff_us), ("B", -10));
        assert_eq!(rows[1].device_number, None);
    }

    #[test]
    fn session_row_text_shows_diff_for_slaves_only() {
        let master = info("M", "/dev/m", None);
        let slave = info("S", "/dev/s", None);
        let rows = session_rows((&master, 100), &[(&slave, 97)]);
        assert_eq!(
            format_session_row(&rows[0]),
            "  M (/dev/m) [master]: clock_count = 100 µs"
        );
        assert_eq!(
            format_session_row(&rows[1]),
            "  S (/dev/s) [slave]: clock_count = 97 µs  (diff: -3 µs)"
        );
    }

    #[test]
    fn session_row_json_has_stable_keys() {
        let master = info("M", "/dev/m", Some(2));
        let json = serde_json::to_value(&session_rows((&master, 5), &[])[0]).unwrap();
        assert_eq!(json["uid"], "M");
        assert_eq!(json["role"], "master");
        assert_eq!(json["clock_count"], 5);
        assert_eq!(json["device_number"], 2);
    }

    #[test]
    fn reconcile_devices_groups_events_across_devices() {
        // Both devices see the same edge; count 2000 vs 2001 straddles a µs.
        let per_device = vec![
            (info("M", "m", None), vec![event(0b1, 2000, 10)]),
            (info("S", "s", None), vec![event(0b1, 2001, 38)]),
            (info("M", "m", None), vec![event(0b0, 9000, 0)]),
        ];
        let (groups, stats) = reconcile_devices(&per_device, 40);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].events.len(), 2);
        assert!(groups[0].reconciled);
        assert_eq!(groups[0].events[0].device, "M");
        assert_eq!(groups[0].events[1].device, "S");
        assert_eq!(groups[1].events.len(), 1);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            (stats[0].a.as_str(), stats[0].b.as_str()),
            ("M:GP0", "S:GP0")
        );
    }

    #[test]
    fn reconcile_devices_without_events_is_empty() {
        let (groups, stats) = reconcile_devices(&[(info("M", "m", None), vec![])], 40);
        assert!(groups.is_empty());
        assert!(stats.is_empty());
    }
}
