//! Command tests against a fake device on a PTY. The fake device replays
//! responses from the golden transcripts (`protocol/golden/`), so the wire
//! text is never written by hand here.

#[path = "commands/safety.rs"]
mod safety;
mod support;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Result;
use pochi::commands::{info, io, map, pattern, raw, timestamp};
use pochi::exit::{exit_code, EXIT_CONNECTION, EXIT_DEVICE_ERROR};
use pochi_sdk::{Connection, PinMap, PushEvent};
use support::{golden_events, FakeDevice, Golden, Script};

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("protocol/fixtures")
        .join(name)
        .display()
        .to_string()
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

const FULL_MAP_SET: &str = "map set flags=0x00 dir=0x0000000f pu=0x000000f0 pd=0x0c7fff00 \
     def=0x00000005 opol=0x00000008 ipol=0x00000100 edge=0x0c7ffff0";

fn full_map_script() -> Script {
    let golden = Golden::load("map.txt");
    Script::new().reply("map get", golden.reply(&[FULL_MAP_SET], "map get"))
}

// --- map get: every item, ch/GP table, masks (R-CLI1) ---

#[test]
fn map_show_prints_all_eight_keys_and_the_channel_table() {
    let (out, _device) = output_of(full_map_script(), |c, o| map::show(c, false, o));
    for expected in [
        "flags: 0x00",
        "dir:   0x0000000f",
        "pu:    0x000000f0",
        "pd:    0x0c7fff00",
        "def:   0x00000005",
        "opol:  0x00000008",
        "ipol:  0x00000100",
        "edge:  0x0c7ffff0",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in\n{out}");
    }
    // The ch/GP table: ch23/ch24 sit on GP26/GP27.
    let row = |ch: &str| {
        out.lines()
            .find(|l| l.starts_with(&format!("{ch} ")))
            .unwrap_or_else(|| panic!("no row for {ch} in\n{out}"))
            .to_string()
    };
    assert!(row("ch0").contains("GP0") && row("ch0").contains("OUT"));
    assert!(row("ch4").contains("GP4") && row("ch4").contains("IN"));
    assert!(row("ch23").contains("GP26"));
    assert!(row("ch24").contains("GP27"));
    assert_eq!(out.lines().filter(|l| l.starts_with("ch")).count(), 25);
}

#[test]
fn map_show_has_no_v1_output() {
    let (out, _device) = output_of(full_map_script(), |c, o| map::show(c, false, o));
    assert!(!out.contains("Map version"));
    assert!(!out.contains("out_base"));
}

#[test]
fn map_show_json_has_all_fields() {
    let (out, _device) = output_of(full_map_script(), |c, o| map::show(c, true, o));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["flags"], 0);
    assert_eq!(json["direction"], 0x0f);
    assert_eq!(json["pull_up"], 0xf0);
    assert_eq!(json["pull_down"], 0x0c7f_ff00);
    assert_eq!(json["out_default"], 5);
    assert_eq!(json["out_polarity"], 8);
    assert_eq!(json["in_polarity"], 0x100);
    assert_eq!(json["edge"], 0x0c7f_fff0);
}

#[test]
fn map_show_of_the_default_map_lists_all_inputs() {
    let golden = Golden::load("map.txt");
    let script = Script::new().reply("map get", golden.reply(&[], "map get"));
    let (out, _device) = output_of(script, |c, o| map::show(c, false, o));
    assert!(out.contains("dir:   0x00000000"));
    assert!(out.contains("pd:    0x0c7fffff"));
    assert!(!out.contains("OUT"));
}

#[test]
fn map_set_sends_the_canonical_map_set_line() {
    let expected = "map set v=2 flags=0x00 dir=0x0000000f pu=0x000000f0 pd=0x0c7fff00 \
                    def=0x00000005 opol=0x00000008 ipol=0x00000100 edge=0x0c7ffff0";
    let args: Vec<String> = [
        "flags=0",
        "dir=15",
        "pu=0xf0",
        "pd=0xC7FFF00",
        "def=0x5",
        "opol=8",
        "ipol=256",
        "edge=0xc7ffff0",
    ]
    .map(String::from)
    .to_vec();
    let map_value = pochi::parse::parse_map_args(&args).unwrap();
    let (out, device) = output_of(Script::new().reply(expected, vec!["OK".into()]), |c, o| {
        map::apply(c, &map_value, "Map set.", false, o)
    });
    assert_eq!(out, "Map set.\n");
    assert_eq!(device.commands().last().unwrap(), expected);
}

#[test]
fn map_load_sends_the_map_of_a_v2_toml_file() {
    let expected = "map set v=2 flags=0x00 dir=0x00000003 pu=0x04000004 pd=0x087ffff0 \
                    def=0x00000002 opol=0x00000001 ipol=0x00000004 edge=0x0c7ffff4";
    let loaded = map::load_map_file(&fixture("map-mixed-io.toml")).unwrap();
    let (_, device) = output_of(Script::new().reply(expected, vec!["OK".into()]), |c, o| {
        map::apply(c, &loaded, "loaded", false, o)
    });
    assert_eq!(device.commands().last().unwrap(), expected);
}

#[test]
fn map_load_rejects_a_bad_file_with_file_and_line() {
    let file = fixture("map-unknown-key.toml");
    let err = map::load_map_file(&file).unwrap_err();
    assert!(
        format!("{err:#}").contains(&format!("{file}:7:")),
        "{err:#}"
    );
}

#[test]
fn map_rejected_by_the_device_is_a_device_error() {
    let errors = Golden::load("errors.txt");
    let bad_map = "map set flags=0x00 dir=0x01000000 pu=0x00000000 pd=0x00000000 \
                   def=0x00000000 opol=0x00000000 ipol=0x00000000 edge=0x00000000";
    // The fake answers every `map set` with the golden ERR BAD_MAP.
    let (mut conn, _device) =
        FakeDevice::connected(Script::new().reply_prefix("map set", errors.reply(&[], bad_map)));
    let err = map::apply(&mut conn, &PinMap::default(), "x", false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), EXIT_DEVICE_ERROR);
    assert!(format!("{err:#}").contains("BAD_MAP"));
}

// --- raw / interactive: all response lines (R-CLI2) ---

#[test]
fn raw_prints_every_line_of_a_counted_response() {
    let help = Golden::load("reset.txt").reply(&[], "help");
    assert!(help.len() > 2, "golden help is multi-line");
    let (out, _device) = output_of(Script::new().reply("help", help.clone()), |c, o| {
        raw::raw(c, "help", false, o)
    });
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        help.iter().map(String::as_str).collect::<Vec<_>>()
    );
}

#[test]
fn raw_json_carries_all_lines() {
    let help = Golden::load("reset.txt").reply(&[], "help");
    let (out, _device) = output_of(Script::new().reply("help", help.clone()), |c, o| {
        raw::raw(c, "help", true, o)
    });
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["lines"].as_array().unwrap().len(), help.len());
}

#[test]
fn raw_shows_a_device_error_line_without_failing() {
    let (out, _device) = output_of(Script::new(), |c, o| raw::raw(c, "frobnicate", false, o));
    assert_eq!(out, "ERR UNKNOWN_CMD\n");
}

#[test]
fn raw_refuses_subscribe_without_contacting_the_device() {
    let (mut conn, device) = FakeDevice::connected(Script::new());
    let err = raw::raw(&mut conn, "subscribe on", false, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("subscribe"));
    assert!(!device.commands().iter().any(|c| c.starts_with("subscribe")));
}

#[test]
fn interactive_prints_multi_line_responses() {
    let help = Golden::load("reset.txt").reply(&[], "help");
    let (mut conn, _device) = FakeDevice::connected(Script::new().reply("help", help.clone()));
    let mut out = Vec::new();
    let mut err = Vec::new();
    raw::repl(
        "help\nfrobnicate\nexit\n".as_bytes(),
        &mut out,
        &mut err,
        |cmd| Ok(conn.raw(cmd)?),
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    for line in &help {
        assert!(out.contains(line.as_str()), "missing {line:?}");
    }
    assert!(out.contains("ERR UNKNOWN_CMD"));
    assert!(err.is_empty());
}

// --- watch: events (R-CLI2) ---

/// Short silence before `watch` pings the device (the real one is longer).
const HEARTBEAT: Duration = Duration::from_millis(200);

fn watch_script() -> Script {
    let events = golden_events("events.txt");
    assert!(events.len() >= 2);
    Script::new()
        .reply("subscribe on", vec!["OK".into()])
        .reply("subscribe off", vec!["OK".into()])
        .then_send("subscribe on", events[..2].to_vec())
        .disconnect_after("subscribe on")
}

#[test]
fn watch_prints_push_events_then_reports_the_disconnect() {
    let (mut conn, _device) = FakeDevice::connected(watch_script());
    let mut out = Vec::new();
    let err = io::watch(
        &mut conn,
        false,
        &AtomicBool::new(false),
        HEARTBEAT,
        &mut out,
    )
    .unwrap_err();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("INPUT in=0x00000001 t=5us"), "{out}");
    assert!(out.contains("INPUT in=0x00000000 t=8us"), "{out}");
    assert_eq!(exit_code(&err), EXIT_CONNECTION);
}

#[test]
fn watch_json_tags_every_event() {
    let (mut conn, _device) = FakeDevice::connected(watch_script());
    let mut out = Vec::new();
    let _ = io::watch(
        &mut conn,
        true,
        &AtomicBool::new(false),
        HEARTBEAT,
        &mut out,
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("\"type\": \"input_change\""), "{out}");
    assert!(out.contains("\"t_us\": 5"), "{out}");
}

#[test]
fn watch_receives_pattern_events() {
    let script = Script::new()
        .reply("subscribe on", vec!["OK".into()])
        .then_send(
            "subscribe on",
            vec![
                "! PATTERN_DONE loops=3".to_string(),
                "! PATTERN_TIMEOUT loops=7".to_string(),
            ],
        )
        .disconnect_after("subscribe on");
    let (mut conn, _device) = FakeDevice::connected(script);
    let events = conn.subscribe_events().unwrap();
    assert_eq!(events.recv().unwrap(), PushEvent::PatternDone { loops: 3 });
    assert_eq!(
        events.recv().unwrap(),
        PushEvent::PatternTimeout { loops: 7 }
    );
}

// --- timestamps ---

const TS_ARM: &str = "ts arm 0x1 rising";

fn ts_script() -> Script {
    let ts = Golden::load("timestamp.txt");
    Script::new().reply("ts get 10", ts.reply(&[TS_ARM], "ts get 10"))
}

#[test]
fn ts_get_shows_events_with_ns_from_the_device_resolution() {
    let (out, _device) = output_of(ts_script(), |c, o| timestamp::get(c, 10, false, false, o));
    assert!(out.contains("Timestamp events: 2"), "{out}");
    assert!(
        out.contains("inputs=0x00000001 1000000ns (c=1000 p=0 s=0)"),
        "{out}"
    );
    assert!(
        out.contains("inputs=0x00000001 1500000ns (c=1500 p=0 s=0)"),
        "{out}"
    );
}

#[test]
fn ts_get_json_includes_ns_and_overflow() {
    let (out, _device) = output_of(ts_script(), |c, o| timestamp::get(c, 10, false, true, o));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["count"], 2);
    assert_eq!(json["overflow"], false);
    assert_eq!(json["events"][0]["timestamp_ns"], 1_000_000);
}

#[test]
fn ts_get_reconcile_groups_the_events() {
    let (out, _device) = output_of(ts_script(), |c, o| timestamp::get(c, 10, true, false, o));
    assert!(out.contains("2 groups"), "{out}");
}

#[test]
fn ts_get_rejects_more_than_the_per_call_maximum_locally() {
    let (mut conn, device) = FakeDevice::connected(Script::new());
    let err = timestamp::get(&mut conn, 256, false, false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), pochi::exit::EXIT_ARGUMENT);
    assert!(!device.commands().iter().any(|c| c.starts_with("ts get")));
}

#[test]
fn ts_get_without_arm_is_a_device_error() {
    let errors = Golden::load("errors.txt");
    let (mut conn, _device) =
        FakeDevice::connected(Script::new().reply("ts get 1", errors.reply(&[], "ts get 1")));
    let err = timestamp::get(&mut conn, 1, false, false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), EXIT_DEVICE_ERROR);
    assert!(format!("{err:#}").contains("NOT_ARMED"));
}

#[test]
fn ts_drain_stops_after_a_short_batch() {
    let (mut conn, device) = FakeDevice::connected(ts_script());
    let drained = timestamp::drain_connection(&mut conn, 10).unwrap();
    assert_eq!(drained.events.len(), 2);
    assert_eq!(device.commands().last().unwrap(), "ts get 10");
}

#[test]
fn ts_drain_reads_full_batches_until_the_ring_is_empty() {
    // A full batch of 85 events (the golden event line repeated), then the
    // golden reply with fewer events than asked for.
    let ts = Golden::load("timestamp.txt");
    let golden_batch = ts.reply(&[TS_ARM], "ts get 10");
    let event_line = golden_batch[1].clone();
    let mut full = vec!["OK count=85 overflow=1".to_string()];
    full.extend(std::iter::repeat_n(event_line, 85));
    let script = Script::new()
        .reply("ts get 85", full)
        .reply("ts get 15", golden_batch);

    let (mut conn, device) = FakeDevice::connected(script);
    let drained = timestamp::drain_connection(&mut conn, 100).unwrap();
    assert_eq!(drained.events.len(), 87);
    assert!(drained.overflow, "overflow of any batch is kept");
    let gets: Vec<String> = device
        .commands()
        .into_iter()
        .filter(|c| c.starts_with("ts get"))
        .collect();
    assert_eq!(gets, ["ts get 85", "ts get 15"]);
}

// --- pattern ---

#[test]
fn pattern_add_sends_tick_steps() {
    let steps = ["0x1:5", "0x0:5"].map(|s| pochi::parse::parse_step(s).unwrap());
    let (out, device) = output_of(
        Script::new().reply("pattern add 0x1:5 0x0:5", vec!["OK total=2".into()]),
        |c, o| pattern::add(c, &steps, false, o),
    );
    assert!(out.contains("Pattern has 2 steps"), "{out}");
    assert_eq!(device.commands().last().unwrap(), "pattern add 0x1:5 0x0:5");
}

#[test]
fn pattern_load_resets_then_adds_the_steps_of_a_toml_file() {
    let file = fixture("pattern-basic.toml");
    let steps = pattern::load_pattern_file(&file).unwrap();
    let add = "pattern add 0x5:5 0x0:10 0x4000000:1";
    let (out, device) = output_of(
        Script::new()
            .reply("pattern reset", vec!["OK".into()])
            .reply(add, vec!["OK total=3".into()]),
        |c, o| pattern::load(c, &steps, &file, false, o),
    );
    assert!(out.contains("3 steps"), "{out}");
    let commands = device.commands();
    let reset = commands.iter().position(|c| c == "pattern reset").unwrap();
    let added = commands.iter().position(|c| c == add).unwrap();
    assert!(reset < added);
}

#[test]
fn pattern_load_rejects_a_bad_file_with_file_and_line() {
    let file = fixture("pattern-unknown-top-key.toml");
    let err = pattern::load_pattern_file(&file).unwrap_err();
    assert!(format!("{err:#}").contains(&format!("{file}:")), "{err:#}");
}

#[test]
fn pattern_status_prints_state_and_total() {
    let golden = Golden::load("pattern.txt");
    let (out, _device) = output_of(
        Script::new().reply("pattern status", golden.reply(&[], "pattern status")),
        |c, o| pattern::status(c, false, o),
    );
    assert!(out.contains("State:        idle"), "{out}");
    assert!(out.contains("Total steps:  0"), "{out}");
}

// --- device information and I/O ---

#[test]
fn version_caps_info_come_from_the_golden_replies() {
    let (out, _d) = output_of(Script::new(), |c, o| info::version(c, false, o));
    assert!(
        out.contains("Firmware: v0.1.3") && out.contains("Protocol: v2"),
        "{out}"
    );

    let (out, _d) = output_of(Script::new(), |c, o| info::caps(c, false, o));
    assert!(out.contains("TS resolution:     40 ns"), "{out}");
    assert!(out.contains("(sync, reset)"), "{out}");

    let (out, _d) = output_of(Script::new(), |c, o| info::info(c, false, o));
    assert!(out.contains("Device number: (unassigned)"), "{out}");
}

#[test]
fn caps_json_uses_the_sdk_field_names() {
    let (out, _d) = output_of(Script::new(), |c, o| info::caps(c, true, o));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["ts_res_ns"], 40);
    assert_eq!(json["tick_ns"], 200);
    assert_eq!(json["in"], 25);
}

#[test]
fn state_and_write_use_typed_values() {
    let golden = Golden::load("io.txt");
    let script = Script::new()
        .reply("state", golden.reply(&[], "state"))
        .reply("write 0xa 0xf", vec!["OK".into()]);
    let (out, _d) = output_of(script, |c, o| io::state(c, false, o));
    assert!(
        out.contains("Inputs:  0x") && out.contains("Outputs: 0x"),
        "{out}"
    );

    let script = Script::new().reply("write 0xa 0xf", vec!["OK".into()]);
    let (out, device) = output_of(script, |c, o| io::write(c, 0x0a, 0x0f, false, o));
    assert_eq!(out, "Output set.\n");
    assert_eq!(device.commands().last().unwrap(), "write 0xa 0xf");
}

#[test]
fn write_outside_the_dio_pins_is_rejected_locally() {
    let (mut conn, device) = FakeDevice::connected(Script::new());
    let err = io::write(&mut conn, 0x80_0000, 0x80_0000, false, &mut Vec::new()).unwrap_err();
    assert_eq!(exit_code(&err), pochi::exit::EXIT_ARGUMENT);
    assert!(!device.commands().iter().any(|c| c.starts_with("write")));
}

#[test]
fn pre_v2_firmware_is_refused_as_a_connection_error() {
    let (conn, _device) =
        FakeDevice::connect(Script::new().reply("version", vec!["OK v0.1.0 proto=1".into()]));
    let err = anyhow::Error::new(conn.err().expect("connection refused"));
    assert_eq!(exit_code(&err), EXIT_CONNECTION);
    assert!(err.to_string().contains("unsupported firmware"));
}
