//! Safety command tests against a fake device on a PTY. Replies that do not
//! depend on a fixture come from the golden transcript `protocol/golden/safety.txt`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::support::{FakeDevice, Golden, Script};
use anyhow::Result;
use pochi::commands::safety;
use pochi::exit::{
    error_json, exit_code, EXIT_ARGUMENT, EXIT_CONNECTION, EXIT_EXCLUSIVE, EXIT_FAILSAFE,
};
use pochi_sdk::{config, Connection};

const SAFETY_IDLE: &str = "OK wdog_ms=0 remaining_ms=0 failsafe=0 cause=none usbsafe=0 excl=0";
const SAFETY_LATCHED: &str = "OK wdog_ms=500 remaining_ms=0 failsafe=1 cause=wdog usbsafe=0 excl=0";
const SAFETY_WDOG_500: &str =
    "OK wdog_ms=500 remaining_ms=480 failsafe=0 cause=none usbsafe=0 excl=0";

/// A poll interval that never fires within a test.
const NEVER: Duration = Duration::from_secs(3600);

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("protocol/fixtures")
        .join(name)
        .display()
        .to_string()
}

fn golden() -> Golden {
    Golden::load("safety.txt")
}

fn ok() -> Vec<String> {
    vec!["OK".to_string()]
}

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(ToString::to_string).collect()
}

/// Run `command` against a device and return what it printed.
fn output_of(
    script: Script,
    command: impl FnOnce(&mut Connection, &mut Vec<u8>) -> Result<()>,
) -> (String, FakeDevice) {
    let (mut conn, device) = FakeDevice::connected(script);
    let mut out = Vec::new();
    command(&mut conn, &mut out).expect("command succeeds");
    (String::from_utf8(out).expect("utf-8 output"), device)
}

fn error_of(
    script: Script,
    command: impl FnOnce(&mut Connection, &mut Vec<u8>) -> Result<()>,
) -> anyhow::Error {
    let (mut conn, _device) = FakeDevice::connected(script);
    command(&mut conn, &mut Vec::new()).expect_err("command fails")
}

fn json_of(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("JSON output")
}

// --- outputs reset ---

#[test]
fn outputs_reset_sends_the_command_and_reports_it() {
    let script = Script::new().reply("outputs reset", golden().reply(&[], "outputs reset"));
    let (out, device) = output_of(script, |c, o| safety::outputs_reset(c, false, o));
    assert!(out.contains("reset"), "{out}");
    assert_eq!(device.commands().last().unwrap(), "outputs reset");
}

#[test]
fn outputs_reset_json_is_status_ok() {
    let script = Script::new().reply("outputs reset", ok());
    let (out, _device) = output_of(script, |c, o| safety::outputs_reset(c, true, o));
    assert_eq!(json_of(&out), serde_json::json!({"status": "ok"}));
}

// --- safety status ---

#[test]
fn status_of_an_idle_device_says_everything_is_off() {
    let script = Script::new().reply("safety", golden().reply(&[], "safety"));
    let (out, _device) = output_of(script, |c, o| safety::status(c, false, o));
    assert!(out.contains("Watchdog:"), "{out}");
    assert!(out.contains("off"), "{out}");
    assert!(out.contains("clear"), "{out}");
    assert!(out.contains("Exclusive groups: 0"), "{out}");
}

#[test]
fn status_shows_a_latched_failsafe_with_its_cause() {
    let script = Script::new().reply("safety", lines(&[SAFETY_LATCHED]));
    let (out, _device) = output_of(script, |c, o| safety::status(c, false, o));
    assert!(out.contains("500 ms"), "{out}");
    assert!(out.contains("LATCHED (cause=wdog)"), "{out}");
}

#[test]
fn status_shows_the_remaining_time_of_a_running_watchdog() {
    let script = Script::new().reply("safety", lines(&[SAFETY_WDOG_500]));
    let (out, _device) = output_of(script, |c, o| safety::status(c, false, o));
    assert!(out.contains("500 ms (480 ms remaining)"), "{out}");
}

#[test]
fn status_json_carries_every_field() {
    let script = Script::new().reply("safety", lines(&[SAFETY_LATCHED]));
    let (out, _device) = output_of(script, |c, o| safety::status(c, true, o));
    assert_eq!(
        json_of(&out),
        serde_json::json!({
            "wdog_ms": 500, "remaining_ms": 0, "failsafe": true,
            "cause": "wdog", "usbsafe": false, "excl": 0
        })
    );
}

// --- wdog / failsafe / usbsafe ---

#[test]
fn wdog_enable_sends_the_timeout_and_warns_about_the_exit() {
    let script = Script::new().reply("wdog 500", ok());
    let (out, device) = output_of(script, |c, o| safety::set_wdog(c, 500, false, o));
    assert_eq!(device.commands().last().unwrap(), "wdog 500");
    assert!(out.contains("500 ms"), "{out}");
    assert!(out.contains("safety hold"), "{out}");
}

#[test]
fn wdog_zero_disables_without_a_warning() {
    let script = Script::new().reply("wdog 0", ok());
    let (out, device) = output_of(script, |c, o| safety::set_wdog(c, 0, false, o));
    assert_eq!(device.commands().last().unwrap(), "wdog 0");
    assert!(out.contains("disabled"), "{out}");
    assert!(!out.contains("safety hold"), "{out}");
}

#[test]
fn wdog_json_reports_the_timeout() {
    let script = Script::new().reply("wdog 800", ok());
    let (out, _device) = output_of(script, |c, o| safety::set_wdog(c, 800, true, o));
    assert_eq!(
        json_of(&out),
        serde_json::json!({"status": "ok", "wdog_ms": 800})
    );
}

#[test]
fn wdog_out_of_range_is_refused_before_anything_is_sent() {
    let (mut conn, device) = FakeDevice::connected(Script::new());
    let before = device.commands().len();
    let err = safety::set_wdog(&mut conn, 99, false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), EXIT_ARGUMENT);
    assert_eq!(device.commands().len(), before);
}

#[test]
fn failsafe_clear_sends_the_command() {
    let script = Script::new().reply("failsafe clear", ok());
    let (out, device) = output_of(script, |c, o| safety::clear_failsafe(c, false, o));
    assert_eq!(device.commands().last().unwrap(), "failsafe clear");
    assert!(out.contains("cleared"), "{out}");
}

#[test]
fn usbsafe_sends_on_and_off() {
    let script = Script::new()
        .reply("usbsafe on", ok())
        .reply("usbsafe off", ok());
    let (mut conn, device) = FakeDevice::connected(script);
    let mut out = Vec::new();
    safety::set_usbsafe(&mut conn, true, false, &mut out).unwrap();
    safety::set_usbsafe(&mut conn, false, false, &mut out).unwrap();
    let sent = device.commands();
    assert_eq!(&sent[sent.len() - 2..], ["usbsafe on", "usbsafe off"]);
}

// --- excl ---

#[test]
fn excl_set_sends_all_masks_in_one_command() {
    let script = Script::new().reply("excl set 0xc 0x18", ok());
    let (out, device) = output_of(script, |c, o| safety::excl_set(c, &[0xc, 0x18], false, o));
    assert_eq!(device.commands().last().unwrap(), "excl set 0xc 0x18");
    assert!(out.contains("2 exclusive group"), "{out}");
}

#[test]
fn excl_set_with_a_single_pin_mask_is_refused_locally() {
    let (mut conn, device) = FakeDevice::connected(Script::new());
    let before = device.commands().len();
    let err = safety::excl_set(&mut conn, &[0x1], false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), EXIT_ARGUMENT);
    assert_eq!(device.commands().len(), before);
}

#[test]
fn excl_clear_sends_the_command() {
    let script = Script::new().reply("excl clear", ok());
    let (_out, device) = output_of(script, |c, o| safety::excl_clear(c, false, o));
    assert_eq!(device.commands().last().unwrap(), "excl clear");
}

#[test]
fn excl_get_lists_every_group_with_its_pins() {
    let reply = golden().reply(&["excl set 0x3 0xc"], "excl get");
    let script = Script::new().reply("excl get", reply);
    let (out, _device) = output_of(script, |c, o| safety::excl_get(c, false, o));
    assert!(
        out.contains("group 0: 0x00000003  GP0 (ch0), GP1 (ch1)"),
        "{out}"
    );
    assert!(
        out.contains("group 1: 0x0000000c  GP2 (ch2), GP3 (ch3)"),
        "{out}"
    );
}

#[test]
fn excl_get_without_groups_says_so() {
    let script = Script::new().reply("excl get", lines(&["OK count=0"]));
    let (out, _device) = output_of(script, |c, o| safety::excl_get(c, false, o));
    assert!(out.contains("No exclusive groups"), "{out}");
}

#[test]
fn excl_get_json_lists_the_masks() {
    let script = Script::new().reply(
        "excl get",
        lines(&["OK count=2", "  mask=0x00000003", "  mask=0x0000000c"]),
    );
    let (out, _device) = output_of(script, |c, o| safety::excl_get(c, true, o));
    assert_eq!(json_of(&out), serde_json::json!({"groups": [3, 12]}));
}

#[test]
fn an_exclusive_violation_has_its_own_exit_code_and_json() {
    let reply = golden().reply(&["excl set 0xc", "write 0x4 0x4"], "write 0x8 0x8");
    assert_eq!(reply, ["ERR EXCLUSIVE group=0 mask=0x0000000c"]);
    let script = Script::new().reply("write 0x8 0x8", reply);
    let err = error_of(script, |c, o| pochi::commands::io::write(c, 8, 8, false, o));
    assert_eq!(exit_code(&err), EXIT_EXCLUSIVE);
    let json = error_json(&err);
    assert_eq!(json["kind"], "exclusive");
    assert_eq!(json["group"], 0);
    assert_eq!(json["mask"], 12);
}

#[test]
fn a_failsafe_refusal_has_its_own_exit_code() {
    // The latch case of the golden: after the trip every write that leaves
    // def is refused.
    let reply = golden().reply(&["wdog 100", "write 0x2 0x2"], "write 0xf 0xf");
    assert_eq!(reply, ["ERR FAILSAFE"]);
    let script = Script::new().reply("write 0xf 0xf", reply);
    let err = error_of(script, |c, o| {
        pochi::commands::io::write(c, 0xf, 0xf, false, o)
    });
    assert_eq!(exit_code(&err), EXIT_FAILSAFE);
    assert_eq!(error_json(&err)["kind"], "failsafe");
}

// --- rstinfo ---

#[test]
fn rstinfo_prints_cause_uptime_and_boot_id() {
    let script = Script::new().reply("rstinfo", golden().reply(&[], "rstinfo"));
    let (out, _device) = output_of(script, |c, o| safety::rstinfo(c, false, o));
    assert!(out.contains("Cause:  por"), "{out}");
    assert!(out.contains("Uptime: 0 ms"), "{out}");
    assert!(out.contains("Boot:   0x00000001"), "{out}");
}

#[test]
fn rstinfo_json_carries_all_fields() {
    let script = Script::new().reply(
        "rstinfo",
        lines(&["OK cause=wdt uptime_ms=1500 boot=0x00000002"]),
    );
    let (out, _device) = output_of(script, |c, o| safety::rstinfo(c, true, o));
    assert_eq!(
        json_of(&out),
        serde_json::json!({"cause": "wdt", "uptime_ms": 1500, "boot": 2})
    );
}

// --- safety apply ---

fn apply_script() -> Script {
    Script::new()
        .reply("excl set 0xc 0x18", ok())
        .reply("usbsafe on", ok())
        .reply("wdog 800", ok())
        .reply(
            "safety",
            lines(&["OK wdog_ms=800 remaining_ms=800 failsafe=0 cause=none usbsafe=1 excl=2"]),
        )
        .reply(
            "excl get",
            lines(&["OK count=2", "  mask=0x0000000c", "  mask=0x00000018"]),
        )
}

#[test]
fn apply_sends_the_commands_of_the_file_in_order_and_reads_back() {
    let config = config::load_safety(fixture("safety-full.toml").as_ref()).unwrap();
    let (mut conn, device) = FakeDevice::connected(apply_script());
    let mut out = Vec::new();
    let state = safety::apply(&mut conn, &config, false, &mut out).unwrap();
    assert_eq!(state.wdog_ms, 800);
    let sent = device.commands();
    assert_eq!(
        &sent[sent.len() - 5..],
        [
            "excl set 0xc 0x18",
            "usbsafe on",
            "wdog 800",
            "safety",
            "excl get"
        ]
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("Safety settings applied"), "{out}");
    assert!(out.contains("safety hold"), "{out}");
}

#[test]
fn apply_of_a_file_without_a_watchdog_has_no_hold_hint() {
    let config = config::load_safety(fixture("safety-minimal.toml").as_ref()).unwrap();
    let script = Script::new()
        .reply("excl clear", ok())
        .reply("usbsafe off", ok())
        .reply("wdog 0", ok())
        .reply("safety", lines(&[SAFETY_IDLE]))
        .reply("excl get", lines(&["OK count=0"]));
    let (mut conn, _device) = FakeDevice::connected(script);
    let mut out = Vec::new();
    safety::apply(&mut conn, &config, false, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("safety hold"), "{out}");
}

#[test]
fn apply_json_carries_the_resulting_state() {
    let config = config::load_safety(fixture("safety-full.toml").as_ref()).unwrap();
    let (mut conn, _device) = FakeDevice::connected(apply_script());
    let mut out = Vec::new();
    safety::apply(&mut conn, &config, true, &mut out).unwrap();
    let json = json_of(&String::from_utf8(out).unwrap());
    assert_eq!(json["status"], "ok");
    assert_eq!(json["safety"]["wdog_ms"], 800);
    assert_eq!(json["safety"]["excl"], 2);
}

#[test]
fn apply_stops_at_the_first_device_error_and_keeps_its_exit_code() {
    let config = config::load_safety(fixture("safety-full.toml").as_ref()).unwrap();
    let script = Script::new().reply(
        "excl set 0xc 0x18",
        lines(&["ERR EXCLUSIVE group=1 mask=0x00000018"]),
    );
    let (mut conn, device) = FakeDevice::connected(script);
    let err = safety::apply(&mut conn, &config, false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), EXIT_EXCLUSIVE);
    assert!(!device.commands().iter().any(|c| c.starts_with("wdog")));
}

#[test]
fn apply_reports_a_readback_mismatch() {
    let config = config::load_safety(fixture("safety-full.toml").as_ref()).unwrap();
    let script = apply_script().reply("safety", lines(&[SAFETY_IDLE]));
    let (mut conn, _device) = FakeDevice::connected(script);
    let err = safety::apply(&mut conn, &config, false, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("wdog_ms"), "{err:#}");
}

#[test]
fn load_config_names_file_and_line_of_a_bad_file() {
    let err = safety::load_config(&fixture("safety-watchdog-too-small.toml")).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("safety-watchdog-too-small.toml:7"), "{text}");
    assert_eq!(exit_code(&err), EXIT_ARGUMENT);
}

#[test]
fn load_config_of_a_missing_file_is_an_input_error() {
    let err = safety::load_config("/no/such/safety.toml").unwrap_err();
    assert_eq!(exit_code(&err), EXIT_ARGUMENT);
    assert!(format!("{err:#}").contains("/no/such/safety.toml"));
}

// --- safety hold ---

#[test]
fn hold_keeps_pinging_until_stopped_and_changes_no_output() {
    let script = Script::new().reply(
        "safety",
        lines(&[SAFETY_WDOG_500.replace("500", "100").as_str()]),
    );
    let (mut conn, device) = FakeDevice::connected(script);
    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            stop.store(true, Ordering::Relaxed);
        });
        safety::hold(&mut conn, &stop, NEVER, false, &mut out).expect("hold ends cleanly");
    });
    let sent = device.commands();
    let pings = sent.iter().filter(|c| c.starts_with("ping ")).count();
    assert!(pings >= 3, "only {pings} pings: {sent:?}");
    for command in &sent {
        assert!(
            matches!(command.as_str(), "version" | "caps" | "info" | "safety")
                || command.starts_with("ping "),
            "hold sent {command:?}"
        );
    }
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("Holding"), "{out}");
    assert!(out.contains("not changed"), "{out}");
}

#[test]
fn hold_json_announces_start_and_stop() {
    let script = Script::new().reply("safety", lines(&[SAFETY_WDOG_500]));
    let (mut conn, _device) = FakeDevice::connected(script);
    let stop = AtomicBool::new(true);
    let mut out = Vec::new();
    safety::hold(&mut conn, &stop, NEVER, true, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let docs: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&text)
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("a stream of JSON documents");
    assert_eq!(docs.len(), 2, "{text}");
    assert_eq!(docs[0]["status"], "holding");
    assert_eq!(docs[0]["wdog_ms"], 500);
    assert_eq!(docs[0]["period_ms"], 125);
    assert_eq!(docs[1]["status"], "stopped");
}

#[test]
fn hold_needs_an_enabled_watchdog() {
    let script = Script::new().reply("safety", lines(&[SAFETY_IDLE]));
    let err = error_of(script, |c, o| {
        safety::hold(c, &AtomicBool::new(true), NEVER, false, o)
    });
    assert!(
        format!("{err:#}").contains("watchdog is disabled"),
        "{err:#}"
    );
    assert_eq!(exit_code(&err), EXIT_ARGUMENT);
}

#[test]
fn hold_refuses_a_device_that_is_already_latched() {
    let script = Script::new().reply("safety", lines(&[SAFETY_LATCHED]));
    let err = error_of(script, |c, o| {
        safety::hold(c, &AtomicBool::new(true), NEVER, false, o)
    });
    assert_eq!(exit_code(&err), EXIT_FAILSAFE);
    assert!(format!("{err:#}").contains("failsafe clear"), "{err:#}");
}

#[test]
fn hold_notices_a_latch_set_while_holding() {
    // The first `safety` read is clean (the hold starts); the device then
    // reports the latch on the next poll.
    let script = Script::new().reply("safety", lines(&[SAFETY_WDOG_500]));
    let (mut conn, device) = FakeDevice::connected(script);
    // After the hold started, the answer changes to a latched state.
    let stop = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            // Wait for a heartbeat ping: it is only sent after the first
            // `safety` was answered (clean), so the latch is seen by a poll
            // during the hold, never by the check at the start.
            while !device.commands().iter().any(|c| c.starts_with("ping ")) {
                std::thread::sleep(Duration::from_millis(5));
            }
            device.set_reply("safety", lines(&[SAFETY_LATCHED]));
        });
        safety::hold(
            &mut conn,
            &stop,
            Duration::from_millis(50),
            false,
            &mut Vec::new(),
        )
    });
    let err = result.unwrap_err();
    assert_eq!(exit_code(&err), EXIT_FAILSAFE);
    assert!(format!("{err:#}").contains("cause=wdog"), "{err:#}");
}

#[test]
fn hold_ends_with_a_connection_error_when_the_device_disappears() {
    // 100 ms timeout: the heartbeat pings every 25 ms; the device hangs up
    // after answering the first ping.
    let script = Script::new()
        .reply(
            "safety",
            lines(&[SAFETY_WDOG_500.replace("500", "100").as_str()]),
        )
        .disconnect_after("ping 0");
    let (mut conn, _device) = FakeDevice::connected(script);
    conn.set_timeout(Duration::from_millis(300));
    let err = safety::hold(
        &mut conn,
        &AtomicBool::new(false),
        NEVER,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(exit_code(&err), EXIT_CONNECTION);
}
