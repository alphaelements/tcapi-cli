//! Safety functions: `outputs reset`, `safety status|apply|hold`, `wdog`,
//! `failsafe clear`, `excl set|clear|get`, `usbsafe` and `rstinfo`.
//!
//! The watchdog, the failsafe latch, the exclusive groups and `usbsafe` live
//! in the device's RAM; this CLI connects per invocation, so a watchdog
//! enabled by one run trips when that process ends. `safety hold` is the
//! long-running command that keeps it fed.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use pochi_sdk::{config, gp_to_ch, Connection, SafetyConfig, SafetyState, SdkError};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::parse_u32;

/// How often `safety hold` checks the stop flag and the heartbeat.
const HOLD_TICK: Duration = Duration::from_millis(100);

/// How often `safety hold` reads `safety` to notice a failsafe trip. The
/// `! FAILSAFE` event can be missed; `safety` is the source of truth.
pub const HOLD_POLL: Duration = Duration::from_secs(1);

/// Report success: `{"status": "ok"}` in JSON mode, `message` otherwise.
fn report(out: &mut impl Write, json: bool, message: &str) -> Result<()> {
    if json {
        output::write_json(out, &serde_json::json!({"status": "ok"}))
    } else {
        writeln!(out, "{message}").context("failed to write output")
    }
}

// --- outputs reset ---

pub fn run_outputs_reset(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    outputs_reset(&mut conn, json, &mut io::stdout().lock())
}

pub fn outputs_reset(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    conn.outputs_reset()?;
    report(out, json, "Outputs reset to their defaults.")
}

// --- safety status ---

pub fn run_status(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    status(&mut conn, json, &mut io::stdout().lock())
}

pub fn status(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let state = conn.safety()?;
    if json {
        output::write_json(out, &state)
    } else {
        write_state(out, &state)
    }
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn write_state(out: &mut impl Write, state: &SafetyState) -> Result<()> {
    if state.wdog_ms == 0 {
        writeln!(out, "Watchdog: off")?;
    } else if state.failsafe {
        writeln!(out, "Watchdog: {} ms", state.wdog_ms)?;
    } else {
        writeln!(
            out,
            "Watchdog: {} ms ({} ms remaining)",
            state.wdog_ms, state.remaining_ms
        )?;
    }
    if state.failsafe {
        writeln!(out, "Failsafe: LATCHED (cause={})", state.cause.as_str())?;
    } else {
        writeln!(out, "Failsafe: clear")?;
    }
    writeln!(out, "USB safe: {}", on_off(state.usbsafe))?;
    writeln!(out, "Exclusive groups: {}", state.excl)?;
    Ok(())
}

// --- safety apply ---

/// Read and parse a `safety.toml` with the SDK parser; errors name the file
/// and line.
pub fn load_config(file: &str) -> Result<SafetyConfig> {
    let src = std::fs::read_to_string(Path::new(file))
        .with_context(|| format!("failed to read safety file: {file}"))?;
    Ok(config::parse_safety(&src, file)?)
}

pub fn run_apply(selector: &DeviceSelector, file: &str, json: bool) -> Result<()> {
    let config = load_config(file)?;
    let (_info, mut conn) = device::open_device(selector)?;
    apply(&mut conn, &config, json, &mut io::stdout().lock()).map(|_| ())
}

/// Make the device equal to `config` (the SDK sends `excl`, `usbsafe` and
/// `wdog` in that order and reads the result back) and report the state.
pub fn apply(
    conn: &mut Connection,
    config: &SafetyConfig,
    json: bool,
    out: &mut impl Write,
) -> Result<SafetyState> {
    let state = conn.apply_safety(config)?;
    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "safety": state}))?;
    } else {
        writeln!(out, "Safety settings applied.")?;
        write_state(out, &state)?;
        if state.wdog_ms > 0 {
            writeln!(out, "{}", watchdog_warning(state.wdog_ms))?;
        }
    }
    Ok(state)
}

/// What an enabled watchdog means for a one-shot command.
fn watchdog_warning(ms: u32) -> String {
    format!(
        "The device trips ({ms} ms without an answered command) when this process ends; \
         run `pochi safety hold` to keep the watchdog fed."
    )
}

// --- safety hold ---

pub fn run_hold(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("failed to set Ctrl+C handler")?;

    hold(&mut conn, &stop, HOLD_POLL, json, &mut io::stdout().lock())
}

/// Fail with [`SdkError::Failsafe`] when the latch is set.
fn ensure_not_latched(state: &SafetyState) -> Result<()> {
    if state.failsafe {
        return Err(anyhow::Error::new(SdkError::Failsafe).context(format!(
            "failsafe latched (cause={}); the outputs are at their defaults, \
             release the latch with `pochi failsafe clear`",
            state.cause.as_str()
        )));
    }
    Ok(())
}

/// Keep the enabled watchdog fed until `stop` is set.
///
/// The heartbeat thread of the SDK sends `ping` every quarter of the
/// timeout; every `poll_every` the safety state is read to notice a trip.
/// Ending (Ctrl+C) sends nothing and changes no output: the watchdog then
/// trips after its timeout, which is the safe outcome. A failed heartbeat
/// (connection lost, timeout) or a latch set while holding ends the command
/// with an error.
pub fn hold(
    conn: &mut Connection,
    stop: &AtomicBool,
    poll_every: Duration,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let state = conn.safety()?;
    if state.wdog_ms == 0 {
        bail!(
            "the watchdog is disabled (wdog_ms=0), nothing to hold; \
             enable it with `pochi wdog <ms>` or `pochi safety apply <file>`"
        );
    }
    ensure_not_latched(&state)?;

    let heartbeat = conn.start_heartbeat(state.wdog_ms)?;
    let period_ms = heartbeat.period().as_millis();
    if json {
        output::write_json(
            out,
            &serde_json::json!({
                "status": "holding", "wdog_ms": state.wdog_ms, "period_ms": period_ms
            }),
        )?;
    } else {
        writeln!(
            out,
            "Holding the watchdog ({} ms), pinging every {period_ms} ms (Ctrl+C to stop)...",
            state.wdog_ms
        )?;
    }
    out.flush().context("failed to write output")?;

    let mut last_poll = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Some(failure) = heartbeat.wait_failure(HOLD_TICK) {
            return Err(anyhow::Error::new(failure)
                .context("heartbeat failed; the device will trip its watchdog"));
        }
        if last_poll.elapsed() >= poll_every {
            ensure_not_latched(&conn.safety()?)?;
            last_poll = Instant::now();
        }
    }
    heartbeat.stop();

    if json {
        output::write_json(out, &serde_json::json!({"status": "stopped"}))
    } else {
        writeln!(
            out,
            "Stopped. Outputs were not changed; the watchdog trips {} ms after the last answered command.",
            state.wdog_ms
        )
        .context("failed to write output")
    }
}

// --- wdog / failsafe / usbsafe ---

pub fn run_wdog(selector: &DeviceSelector, ms: u32, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    set_wdog(&mut conn, ms, json, &mut io::stdout().lock())
}

pub fn set_wdog(conn: &mut Connection, ms: u32, json: bool, out: &mut impl Write) -> Result<()> {
    conn.wdog(ms)?;
    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "wdog_ms": ms}))
    } else if ms == 0 {
        writeln!(out, "Watchdog disabled.").context("failed to write output")
    } else {
        writeln!(out, "Watchdog set to {ms} ms.")?;
        writeln!(out, "{}", watchdog_warning(ms)).context("failed to write output")
    }
}

pub fn run_failsafe_clear(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    clear_failsafe(&mut conn, json, &mut io::stdout().lock())
}

pub fn clear_failsafe(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    conn.failsafe_clear()?;
    report(out, json, "Failsafe latch cleared.")
}

pub fn run_usbsafe(selector: &DeviceSelector, on: bool, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    set_usbsafe(&mut conn, on, json, &mut io::stdout().lock())
}

pub fn set_usbsafe(
    conn: &mut Connection,
    on: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    conn.usbsafe(on)?;
    report(out, json, &format!("USB safe {}.", on_off(on)))
}

// --- excl ---

pub fn run_excl_set(selector: &DeviceSelector, masks: &[String], json: bool) -> Result<()> {
    let masks = masks
        .iter()
        .map(|m| parse_u32(m).with_context(|| format!("invalid group mask {m:?}")))
        .collect::<Result<Vec<_>>>()?;
    let (_info, mut conn) = device::open_device(selector)?;
    excl_set(&mut conn, &masks, json, &mut io::stdout().lock())
}

pub fn excl_set(
    conn: &mut Connection,
    masks: &[u32],
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    conn.excl_set(masks)?;
    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "groups": masks}))
    } else {
        let plural = if masks.len() == 1 { "" } else { "s" };
        writeln!(out, "{} exclusive group{plural} set.", masks.len())
            .context("failed to write output")
    }
}

pub fn run_excl_clear(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    excl_clear(&mut conn, json, &mut io::stdout().lock())
}

pub fn excl_clear(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    conn.excl_clear()?;
    report(out, json, "Exclusive groups cleared.")
}

pub fn run_excl_get(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    excl_get(&mut conn, json, &mut io::stdout().lock())
}

/// `GP2 (ch2), GP3 (ch3)` for the pins of `mask`.
fn pin_labels(mask: u32) -> Result<String> {
    let labels = (0..32u8)
        .filter(|gp| mask & (1 << gp) != 0)
        .map(|gp| Ok(format!("GP{gp} (ch{})", gp_to_ch(gp)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(labels.join(", "))
}

pub fn excl_get(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let groups = conn.excl_get()?;
    if json {
        return output::write_json(out, &serde_json::json!({"groups": groups}));
    }
    if groups.is_empty() {
        return writeln!(out, "No exclusive groups.").context("failed to write output");
    }
    for (i, mask) in groups.iter().enumerate() {
        writeln!(out, "group {i}: {mask:#010x}  {}", pin_labels(*mask)?)?;
    }
    Ok(())
}

// --- rstinfo ---

pub fn run_rstinfo(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    rstinfo(&mut conn, json, &mut io::stdout().lock())
}

pub fn rstinfo(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let info = conn.rstinfo()?;
    if json {
        output::write_json(out, &info)
    } else {
        writeln!(out, "Cause:  {}", info.cause.as_str())?;
        writeln!(out, "Uptime: {} ms", info.uptime_ms)?;
        writeln!(out, "Boot:   {:#010x}", info.boot).context("failed to write output")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_labels_name_gp_and_channel() {
        assert_eq!(pin_labels(0b1100).unwrap(), "GP2 (ch2), GP3 (ch3)");
        assert_eq!(pin_labels(1 << 26).unwrap(), "GP26 (ch23)");
        assert_eq!(pin_labels(0).unwrap(), "");
    }
}
