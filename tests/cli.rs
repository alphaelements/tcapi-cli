//! Tests of the `tcapi` binary that do not need a device: argument errors,
//! TOML file errors and exit codes. Everything here fails (or succeeds)
//! before a connection is opened.

use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

/// A serial port path that does not exist.
const NO_PORT: &str = "/dev/tcapi-test-no-such-port";

fn tcapi(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("tcapi").expect("tcapi binary");
    cmd.args(["--port", NO_PORT])
        .args(args)
        .timeout(Duration::from_secs(20));
    cmd
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("protocol/fixtures")
        .join(name)
        .display()
        .to_string()
}

#[test]
fn unreachable_port_is_a_connection_error() {
    tcapi(&["read"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to open"));
}

#[test]
fn json_mode_reports_errors_on_stdout() {
    tcapi(&["--json", "read"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"error\""));
}

#[test]
fn usage_errors_exit_with_the_argument_code() {
    Command::cargo_bin("tcapi")
        .unwrap()
        .arg("no-such-command")
        .assert()
        .code(3);
}

#[test]
fn help_and_version_succeed() {
    Command::cargo_bin("tcapi")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Pin map operations"));
    Command::cargo_bin("tcapi")
        .unwrap()
        .arg("--version")
        .assert()
        .success();
}

#[test]
fn map_set_needs_all_eight_keys_before_any_connection() {
    tcapi(&["map", "set", "dir=0x1"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("missing map key"));
}

#[test]
fn map_set_rejects_an_invalid_map_before_any_connection() {
    // pull-up and pull-down on the same pin (R-M1)
    tcapi(&[
        "map", "set", "flags=0", "dir=0", "pu=0x1", "pd=0x1", "def=0", "opol=0", "ipol=0", "edge=0",
    ])
    .assert()
    .code(3)
    .stderr(predicate::str::contains("pull"));
}

#[test]
fn map_load_reports_file_and_line_of_a_bad_file() {
    let file = fixture("map-unknown-key.toml");
    tcapi(&["map", "load", &file])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!("{file}:7:")));
}

#[test]
fn map_load_rejects_a_missing_file() {
    tcapi(&["map", "load", "/nonexistent/map.toml"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("failed to read map file"));
}

#[test]
fn map_load_rejects_a_v1_toml_file() {
    let dir = std::env::temp_dir().join(format!("tcapi-cli-v1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("v1.toml");
    std::fs::write(&file, "version = 1\n[input]\nbase = 0\ncount = 23\n").unwrap();
    tcapi(&["map", "load", file.to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("v1.toml:1:"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pattern_upload_reports_file_and_line_of_a_bad_file() {
    let file = fixture("pattern-unknown-top-key.toml");
    for subcommand in ["upload", "load"] {
        tcapi(&["pattern", subcommand, &file])
            .assert()
            .code(3)
            .stderr(predicate::str::contains(format!("{file}:")));
    }
}

#[test]
fn pattern_add_rejects_a_malformed_step() {
    tcapi(&["pattern", "add", "0x1"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("<out_bits>:<hold_ticks>"));
}

#[test]
fn v1_hex_pattern_load_is_gone() {
    tcapi(&["pattern", "load-hex", "0100"]).assert().code(3);
}

#[test]
fn ts_arm_rejects_an_unknown_edge_mode() {
    tcapi(&["ts", "arm", "0x1", "sideways"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("rising, falling or both"));
}

#[test]
fn devnum_rejects_255() {
    tcapi(&["devnum", "255"]).assert().code(3);
}

fn help_of(args: &[&str]) -> String {
    let out = Command::cargo_bin("tcapi")
        .unwrap()
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    String::from_utf8(out.stdout).expect("utf-8 help")
}

#[test]
fn top_level_help_lists_the_safety_commands() {
    let help = help_of(&["--help"]);
    for command in [
        "outputs", "safety", "wdog", "failsafe", "excl", "usbsafe", "rstinfo",
    ] {
        assert!(
            help.lines().any(|l| l.trim_start().starts_with(command)),
            "{command} missing from\n{help}"
        );
    }
}

#[test]
fn outputs_has_reset_and_no_alloff() {
    let help = help_of(&["outputs", "--help"]);
    assert!(help.contains("reset"), "{help}");
    assert!(!help.to_lowercase().contains("alloff"), "{help}");
    Command::cargo_bin("tcapi")
        .unwrap()
        .args(["outputs", "alloff"])
        .assert()
        .code(3);
}

#[test]
fn wdog_help_says_it_trips_when_the_process_ends() {
    let help = help_of(&["wdog", "--help"]);
    assert!(
        help.contains("trips the device once this process has ended"),
        "{help}"
    );
    assert!(help.contains("safety hold"), "{help}");
}

#[test]
fn safety_subcommands_document_the_watchdog_caveat() {
    let apply = help_of(&["safety", "apply", "--help"]);
    assert!(
        apply.contains("excl set") && apply.contains("safety hold"),
        "{apply}"
    );
    let hold = help_of(&["safety", "hold", "--help"]);
    assert!(
        hold.contains("Ctrl+C") && hold.contains("without changing"),
        "{hold}"
    );
}

#[test]
fn wdog_out_of_range_is_an_argument_error_before_any_connection() {
    tcapi(&["wdog", "99"]).assert().code(3);
    tcapi(&["wdog", "abc"]).assert().code(3);
}

#[test]
fn excl_set_needs_at_least_one_mask() {
    tcapi(&["excl", "set"]).assert().code(3);
}

#[test]
fn safety_apply_rejects_a_bad_file_before_any_connection() {
    tcapi(&["safety", "apply", &fixture("safety-pin-duplicate.toml")])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("safety-pin-duplicate.toml"));
}
