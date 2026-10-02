//! Tests of the `pochi` binary that do not need a device: argument errors,
//! TOML file errors and exit codes. Everything here fails (or succeeds)
//! before a connection is opened.

use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

/// A serial port path that does not exist.
const NO_PORT: &str = "/dev/pochi-test-no-such-port";

fn pochi(args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("pochi").expect("pochi binary");
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
    pochi(&["read"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to open"));
}

#[test]
fn json_mode_reports_errors_on_stdout() {
    pochi(&["--json", "read"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"error\""));
}

#[test]
fn usage_errors_exit_with_the_argument_code() {
    Command::cargo_bin("pochi")
        .unwrap()
        .arg("no-such-command")
        .assert()
        .code(3);
}

#[test]
fn help_and_version_succeed() {
    Command::cargo_bin("pochi")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Pin map operations"));
    Command::cargo_bin("pochi")
        .unwrap()
        .arg("--version")
        .assert()
        .success();
}

#[test]
fn map_set_needs_all_eight_keys_before_any_connection() {
    pochi(&["map", "set", "dir=0x1"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("missing map key"));
}

#[test]
fn map_set_rejects_an_invalid_map_before_any_connection() {
    // pull-up and pull-down on the same pin (R-M1)
    pochi(&[
        "map", "set", "flags=0", "dir=0", "pu=0x1", "pd=0x1", "def=0", "opol=0", "ipol=0", "edge=0",
    ])
    .assert()
    .code(3)
    .stderr(predicate::str::contains("pull"));
}

#[test]
fn map_load_reports_file_and_line_of_a_bad_file() {
    let file = fixture("map-unknown-key.toml");
    pochi(&["map", "load", &file])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!("{file}:7:")));
}

#[test]
fn map_load_rejects_a_missing_file() {
    pochi(&["map", "load", "/nonexistent/map.toml"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("failed to read map file"));
}

#[test]
fn map_load_rejects_a_v1_toml_file() {
    let dir = std::env::temp_dir().join(format!("pochi-cli-v1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("v1.toml");
    std::fs::write(&file, "version = 1\n[input]\nbase = 0\ncount = 23\n").unwrap();
    pochi(&["map", "load", file.to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("v1.toml:1:"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pattern_upload_reports_file_and_line_of_a_bad_file() {
    let file = fixture("pattern-unknown-top-key.toml");
    for subcommand in ["upload", "load"] {
        pochi(&["pattern", subcommand, &file])
            .assert()
            .code(3)
            .stderr(predicate::str::contains(format!("{file}:")));
    }
}

#[test]
fn pattern_add_rejects_a_malformed_step() {
    pochi(&["pattern", "add", "0x1"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("<out_bits>:<hold_ticks>"));
}

#[test]
fn v1_hex_pattern_load_is_gone() {
    pochi(&["pattern", "load-hex", "0100"]).assert().code(3);
}

#[test]
fn ts_arm_rejects_an_unknown_edge_mode() {
    pochi(&["ts", "arm", "0x1", "sideways"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("rising, falling or both"));
}

#[test]
fn devnum_rejects_255() {
    pochi(&["devnum", "255"]).assert().code(3);
}
