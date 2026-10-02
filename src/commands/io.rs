use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pochi_sdk::{Connection, PushEvent, SdkError};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::parse_u32;

/// How often the watch loop checks the Ctrl+C flag while no event arrives.
const WATCH_POLL: Duration = Duration::from_millis(100);

/// Idle time after which `watch` pings the device to notice an unplug.
pub const WATCH_HEARTBEAT: Duration = Duration::from_secs(2);

/// Value echoed by the watch heartbeat `ping`.
const WATCH_PING_VALUE: u32 = 0;

pub fn run_read(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    read(&mut conn, json, &mut io::stdout().lock())
}

pub fn read(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let inputs = conn.read()?;

    if json {
        output::write_json(out, &serde_json::json!({ "inputs": inputs }))?;
    } else {
        writeln!(out, "Inputs: {inputs:#010x}")?;
    }
    Ok(())
}

pub fn run_write(selector: &DeviceSelector, values: &str, mask: &str, json: bool) -> Result<()> {
    let values = parse_u32(values).context("invalid output values")?;
    let mask = parse_u32(mask).context("invalid update mask")?;
    let (_info, mut conn) = device::open_device(selector)?;
    write(&mut conn, values, mask, json, &mut io::stdout().lock())
}

pub fn write(
    conn: &mut Connection,
    values: u32,
    mask: u32,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    conn.write(values, mask)?;

    if json {
        output::write_json(out, &serde_json::json!({"status": "ok"}))?;
    } else {
        writeln!(out, "Output set.")?;
    }
    Ok(())
}

pub fn run_state(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    state(&mut conn, json, &mut io::stdout().lock())
}

pub fn state(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let state = conn.state()?;

    if json {
        output::write_json(out, &state)?;
    } else {
        writeln!(out, "Inputs:  {:#010x}", state.inputs)?;
        writeln!(out, "Outputs: {:#010x}", state.outputs)?;
    }
    Ok(())
}

/// One push event as a human-readable line.
pub fn format_event(event: &PushEvent) -> String {
    match event {
        PushEvent::InputChange { inputs, t_us } => {
            format!("INPUT in={inputs:#010x} t={t_us}us")
        }
        PushEvent::PatternDone { loops } => format!("PATTERN_DONE loops={loops}"),
        PushEvent::PatternTimeout { loops } => format!("PATTERN_TIMEOUT loops={loops}"),
        PushEvent::Unknown { raw } => format!("UNKNOWN {raw}"),
    }
}

/// Hand every event from `events` to `emit` until `stop` is set.
///
/// The event channel cannot tell that the device is gone (it only closes when
/// the connection object is dropped), so while no event arrives for
/// `heartbeat_every` the `heartbeat` check runs; its error ends the loop. A
/// closed event channel is [`SdkError::Disconnected`] as well, not a clean
/// end.
pub fn watch_loop(
    events: &Receiver<PushEvent>,
    stop: &AtomicBool,
    heartbeat_every: Duration,
    mut heartbeat: impl FnMut() -> Result<()>,
    mut emit: impl FnMut(&PushEvent) -> Result<()>,
) -> Result<()> {
    let mut last_activity = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match events.recv_timeout(WATCH_POLL) {
            Ok(event) => {
                emit(&event)?;
                last_activity = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_activity.elapsed() >= heartbeat_every {
                    heartbeat()?;
                    last_activity = Instant::now();
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Err(SdkError::Disconnected.into()),
        }
    }
    Ok(())
}

pub fn run_watch(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("failed to set Ctrl+C handler")?;

    watch(
        &mut conn,
        json,
        &stop,
        WATCH_HEARTBEAT,
        &mut io::stdout().lock(),
    )
}

/// Subscribe to push events and print each one until `stop` is set or the
/// connection is lost (noticed by a ping after `heartbeat_every` of silence).
pub fn watch(
    conn: &mut Connection,
    json: bool,
    stop: &AtomicBool,
    heartbeat_every: Duration,
    out: &mut impl Write,
) -> Result<()> {
    let events = conn.subscribe_events()?;

    if !json {
        writeln!(out, "Watching for device events (Ctrl+C to stop)...")?;
        out.flush().context("failed to write output")?;
    }

    watch_loop(
        &events,
        stop,
        heartbeat_every,
        || {
            conn.ping(WATCH_PING_VALUE)?;
            Ok(())
        },
        |event| {
            if json {
                output::write_json(out, event)?;
            } else {
                writeln!(out, "{}", format_event(event))?;
            }
            out.flush().context("failed to write output")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Heartbeat interval that never fires within a test.
    const IDLE: Duration = Duration::from_secs(3600);

    #[test]
    fn events_are_formatted_in_wire_style() {
        assert_eq!(
            format_event(&PushEvent::InputChange {
                inputs: 5,
                t_us: 42
            }),
            "INPUT in=0x00000005 t=42us"
        );
        assert_eq!(
            format_event(&PushEvent::PatternDone { loops: 3 }),
            "PATTERN_DONE loops=3"
        );
        assert_eq!(
            format_event(&PushEvent::PatternTimeout { loops: 7 }),
            "PATTERN_TIMEOUT loops=7"
        );
        assert_eq!(
            format_event(&PushEvent::Unknown {
                raw: "FOO a=1".into()
            }),
            "UNKNOWN FOO a=1"
        );
    }

    #[test]
    fn watch_loop_delivers_events_in_order_and_reports_disconnect() {
        let (tx, rx) = mpsc::channel();
        tx.send(PushEvent::InputChange { inputs: 1, t_us: 5 })
            .unwrap();
        tx.send(PushEvent::PatternDone { loops: 2 }).unwrap();
        drop(tx);

        let mut seen = Vec::new();
        let result = watch_loop(
            &rx,
            &AtomicBool::new(false),
            IDLE,
            || Ok(()),
            |event| {
                seen.push(event.clone());
                Ok(())
            },
        );

        assert_eq!(
            seen,
            vec![
                PushEvent::InputChange { inputs: 1, t_us: 5 },
                PushEvent::PatternDone { loops: 2 },
            ]
        );
        let err = result.unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SdkError>(),
            Some(SdkError::Disconnected)
        ));
    }

    #[test]
    fn watch_loop_stops_cleanly_when_flag_is_set() {
        let (tx, rx) = mpsc::channel::<PushEvent>();
        let stop = AtomicBool::new(true);
        assert!(watch_loop(&rx, &stop, IDLE, || Ok(()), |_| Ok(())).is_ok());
        drop(tx);
    }

    #[test]
    fn watch_loop_propagates_emit_errors() {
        let (tx, rx) = mpsc::channel();
        tx.send(PushEvent::PatternDone { loops: 1 }).unwrap();
        let result = watch_loop(
            &rx,
            &AtomicBool::new(false),
            IDLE,
            || Ok(()),
            |_| Err(anyhow::anyhow!("sink closed")),
        );
        assert_eq!(result.unwrap_err().to_string(), "sink closed");
    }

    #[test]
    fn watch_loop_runs_the_heartbeat_when_idle_and_stops_on_its_error() {
        let (tx, rx) = mpsc::channel::<PushEvent>();
        let mut beats = 0;
        let result = watch_loop(
            &rx,
            &AtomicBool::new(false),
            Duration::from_millis(150),
            || {
                beats += 1;
                if beats == 2 {
                    Err(SdkError::Timeout.into())
                } else {
                    Ok(())
                }
            },
            |_| Ok(()),
        );
        assert_eq!(beats, 2);
        assert!(matches!(
            result.unwrap_err().downcast_ref::<SdkError>(),
            Some(SdkError::Timeout)
        ));
        drop(tx);
    }

    #[test]
    fn watch_loop_does_not_heartbeat_while_events_keep_arriving() {
        let (tx, rx) = mpsc::channel();
        for loops in 0..3 {
            tx.send(PushEvent::PatternDone { loops }).unwrap();
        }
        drop(tx);
        let mut beats = 0;
        let _ = watch_loop(
            &rx,
            &AtomicBool::new(false),
            Duration::from_secs(3600),
            || {
                beats += 1;
                Ok(())
            },
            |_| Ok(()),
        );
        assert_eq!(beats, 0);
    }
}
