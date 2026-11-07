//! The real `pochi` binary against the firmware simulator `pochi-dio-sim`
//! (pochi-dio/sim, a pty running the firmware's protocol core on wall-clock
//! time). These tests start the actual binary, so exit codes, `--json`
//! output and the watchdog timing are the ones a user sees.
//!
//! The simulator comes from `POCHI_DIO_SIM` (path to the binary) or is built
//! from the sibling `../pochi-dio/sim` checkout. Without either (pochi-dio is
//! a private repository) the tests print `SKIPPED` and pass.
//!
//! The SDK asserts DTR when it opens a port by path, which a pty answers with
//! `ENOTTY`; `support/dtr_shim.c` is preloaded into the `pochi` process to
//! accept that one request. Linux only.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

const POCHI: &str = env!("CARGO_BIN_EXE_pochi");

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> String {
    manifest_dir()
        .join("protocol/fixtures")
        .join(name)
        .display()
        .to_string()
}

fn run_checked(command: &mut Process, what: &str) {
    let output = command.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(
        output.status.success(),
        "{what} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The simulator binary, or `None` when there is no way to get one.
fn sim_binary() -> Option<&'static Path> {
    static SIM: OnceLock<Option<PathBuf>> = OnceLock::new();
    SIM.get_or_init(|| {
        if let Some(path) = std::env::var_os("POCHI_DIO_SIM") {
            let path = PathBuf::from(path);
            assert!(path.is_file(), "POCHI_DIO_SIM {path:?} is not a file");
            return Some(path);
        }
        let sim_dir = manifest_dir().join("../pochi-dio/sim");
        if !sim_dir.join("Cargo.toml").is_file() {
            return None;
        }
        let host = host_triple();
        let target_dir = manifest_dir().join("target/pochi-dio-sim");
        run_checked(
            Process::new("cargo")
                .args(["build", "--quiet", "--manifest-path"])
                .arg(sim_dir.join("Cargo.toml"))
                .args(["--target", &host, "--target-dir"])
                .arg(&target_dir),
            "building pochi-dio-sim",
        );
        Some(target_dir.join(&host).join("debug/pochi-dio-sim"))
    })
    .as_deref()
}

fn host_triple() -> String {
    let output = Process::new("rustc")
        .arg("-vV")
        .output()
        .expect("run rustc -vV");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("host line in rustc -vV")
        .to_string()
}

/// The preload library that accepts the pty's missing modem lines.
fn dtr_shim() -> &'static Path {
    static SHIM: OnceLock<PathBuf> = OnceLock::new();
    SHIM.get_or_init(|| {
        let dir = manifest_dir().join("target/pochi-sim-shim");
        std::fs::create_dir_all(&dir).expect("create shim dir");
        let lib = dir.join("dtr_shim.so");
        // Compile beside the target and rename: concurrent test runs never
        // see a half-written library.
        let building = dir.join(format!("dtr_shim-{}.so", std::process::id()));
        run_checked(
            Process::new("cc")
                .args(["-shared", "-fPIC", "-O2", "-o"])
                .arg(&building)
                .arg(manifest_dir().join("tests/support/dtr_shim.c"))
                .arg("-ldl"),
            "compiling the DTR shim (needs a C compiler)",
        );
        std::fs::rename(&building, &lib).expect("install the DTR shim");
        lib
    })
}

/// A running simulator; killed when dropped.
struct Sim {
    child: Child,
    port: String,
}

impl Sim {
    fn start() -> Option<Self> {
        let Some(binary) = sim_binary() else {
            eprintln!("SKIPPED: no pochi-dio-sim (set POCHI_DIO_SIM or check out pochi-dio)");
            return None;
        };
        let mut child = Process::new(binary)
            .arg("--wall-clock")
            .stdout(Stdio::piped())
            .spawn()
            .expect("start pochi-dio-sim");
        let mut port = String::new();
        BufReader::new(child.stdout.as_mut().expect("sim stdout"))
            .read_line(&mut port)
            .expect("read the pty path");
        let port = port.trim().to_string();
        assert!(port.starts_with('/'), "unexpected pty path {port:?}");
        Some(Self { child, port })
    }

    fn pochi(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(POCHI);
        cmd.env("LD_PRELOAD", dtr_shim())
            .args(["--port", &self.port])
            .args(args)
            .timeout(Duration::from_secs(20));
        cmd
    }

    /// Run `pochi args` and parse its `--json` stdout; it must succeed.
    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.pochi(&all).assert().success().get_output().clone();
        serde_json::from_slice(&out.stdout).expect("JSON on stdout")
    }

    /// Output pins GP0-GP3 (default level 0) and GP4 as outputs.
    fn map_outputs(&self, dir: &str) {
        let pd = format!(
            "{:#x}",
            0x0c7f_ffffu32 & !u32::from_str_radix(&dir[2..], 16).unwrap()
        );
        self.pochi(&[
            "map",
            "set",
            "flags=0",
            &format!("dir={dir}"),
            "pu=0",
            &format!("pd={pd}"),
            "def=0",
            "opol=0",
            "ipol=0",
            "edge=0",
        ])
        .assert()
        .success();
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

macro_rules! sim {
    () => {
        match Sim::start() {
            Some(sim) => sim,
            None => return,
        }
    };
}

#[test]
fn the_binary_talks_to_the_simulator() {
    let sim = sim!();
    sim.pochi(&["version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Protocol: v2"));
}

#[test]
fn rstinfo_reports_a_power_on_reset() {
    let sim = sim!();
    let info = sim.json(&["rstinfo"]);
    assert_eq!(info["cause"], "por");
    assert_eq!(info["boot"], 1);
}

#[test]
fn safety_status_starts_all_off() {
    let sim = sim!();
    let state = sim.json(&["safety", "status"]);
    assert_eq!(
        state,
        serde_json::json!({
            "wdog_ms": 0, "remaining_ms": 0, "failsafe": false,
            "cause": "none", "usbsafe": false, "excl": 0
        })
    );
}

#[test]
fn outputs_reset_returns_the_outputs_to_def() {
    let sim = sim!();
    sim.map_outputs("0xf");
    sim.pochi(&["write", "0x5", "0xf"]).assert().success();
    assert_eq!(sim.json(&["state"])["outputs"], 5);
    sim.pochi(&["outputs", "reset"]).assert().success();
    assert_eq!(sim.json(&["state"])["outputs"], 0);
}

#[test]
fn a_watchdog_left_unfed_trips_and_the_latch_has_its_exit_code() {
    let sim = sim!();
    sim.map_outputs("0xf");
    sim.pochi(&["write", "0x1", "0x1"]).assert().success();
    sim.pochi(&["wdog", "200"])
        .assert()
        .success()
        .stdout(predicate::str::contains("safety hold"));
    sleep(Duration::from_millis(600));

    let state = sim.json(&["safety", "status"]);
    assert_eq!(state["failsafe"], true);
    assert_eq!(state["cause"], "wdog");
    // The trip put the outputs at def.
    assert_eq!(sim.json(&["state"])["outputs"], 0);

    // The latch refuses a write that leaves def: exit code 4, kind in JSON.
    sim.pochi(&["write", "0x1", "0x1"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("FAILSAFE"));
    let out = sim
        .pochi(&["--json", "write", "0x1", "0x1"])
        .assert()
        .code(4)
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "failsafe");

    // outputs reset works under the latch and keeps it.
    sim.pochi(&["outputs", "reset"]).assert().success();
    assert_eq!(sim.json(&["safety", "status"])["failsafe"], true);

    sim.pochi(&["failsafe", "clear"]).assert().success();
    sim.pochi(&["wdog", "off"]).assert().success();
    sim.pochi(&["write", "0x1", "0x1"]).assert().success();
}

#[test]
fn an_exclusive_violation_exits_5_with_group_and_mask() {
    let sim = sim!();
    sim.map_outputs("0xf");
    sim.pochi(&["excl", "set", "0x3", "0xc"]).assert().success();
    sim.pochi(&["write", "0x4", "0x4"]).assert().success();

    sim.pochi(&["write", "0x8", "0x8"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("EXCLUSIVE"));
    let out = sim
        .pochi(&["--json", "write", "0x8", "0x8"])
        .assert()
        .code(5)
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "exclusive");
    assert_eq!(json["group"], 1);
    assert_eq!(json["mask"], 12);

    assert_eq!(
        sim.json(&["excl", "get"])["groups"],
        serde_json::json!([3, 12])
    );
    sim.pochi(&["excl", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "group 1: 0x0000000c  GP2 (ch2), GP3 (ch3)",
        ));

    // outputs reset ignores the groups; excl clear lifts the restriction.
    sim.pochi(&["outputs", "reset"]).assert().success();
    sim.pochi(&["excl", "clear"]).assert().success();
    sim.pochi(&["write", "0xc", "0xc"]).assert().success();
}

#[test]
fn excl_set_of_a_non_output_pin_is_a_device_error() {
    let sim = sim!();
    sim.map_outputs("0x3");
    sim.pochi(&["excl", "set", "0xc"]).assert().code(1);
}

#[test]
fn usbsafe_is_set_and_shown() {
    let sim = sim!();
    sim.pochi(&["usbsafe", "on"]).assert().success();
    assert_eq!(sim.json(&["safety", "status"])["usbsafe"], true);
    sim.pochi(&["usbsafe", "off"]).assert().success();
    assert_eq!(sim.json(&["safety", "status"])["usbsafe"], false);
}

#[test]
fn safety_apply_sets_everything_in_the_file_and_verifies_it() {
    let sim = sim!();
    sim.map_outputs("0x1f");
    let applied = sim.json(&["safety", "apply", &fixture("safety-full.toml")]);
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["safety"]["wdog_ms"], 800);

    let state = sim.json(&["safety", "status"]);
    assert_eq!(state["usbsafe"], true);
    assert_eq!(state["excl"], 2);
    assert_eq!(
        sim.json(&["excl", "get"])["groups"],
        serde_json::json!([0xc, 0x18])
    );
}

#[test]
fn safety_apply_of_a_bad_file_is_an_argument_error_with_file_and_line() {
    let sim = sim!();
    sim.pochi(&[
        "safety",
        "apply",
        &fixture("safety-watchdog-too-small.toml"),
    ])
    .assert()
    .code(3)
    .stderr(predicate::str::contains("safety-watchdog-too-small.toml:7"));
}

#[test]
fn safety_apply_with_groups_on_input_pins_stops_with_a_device_error() {
    let sim = sim!();
    // GP2-GP4 are inputs after the default map: the device refuses the groups
    // and nothing after `excl set` is sent.
    sim.pochi(&["safety", "apply", &fixture("safety-full.toml")])
        .assert()
        .code(1);
    assert_eq!(sim.json(&["safety", "status"])["wdog_ms"], 0);
}

#[test]
fn safety_hold_needs_an_enabled_watchdog() {
    let sim = sim!();
    sim.pochi(&["safety", "hold"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("watchdog is disabled"));
}

#[test]
fn safety_hold_keeps_the_watchdog_fed_and_sigint_leaves_the_outputs_alone() {
    let sim = sim!();
    sim.map_outputs("0xf");
    sim.pochi(&["write", "0x1", "0x1"]).assert().success();
    sim.pochi(&["wdog", "1000"]).assert().success();

    let mut hold = Process::new(POCHI)
        .env("LD_PRELOAD", dtr_shim())
        .args(["--port", &sim.port, "safety", "hold"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pochi safety hold");
    let mut stdout = BufReader::new(hold.stdout.take().expect("hold stdout"));
    let mut first = String::new();
    stdout.read_line(&mut first).expect("first line");
    assert!(
        first.starts_with("Holding the watchdog (1000 ms)"),
        "{first}"
    );

    // Longer than the timeout: without the heartbeat the device would trip.
    sleep(Duration::from_millis(1600));
    run_checked(
        Process::new("kill").args(["-INT", &hold.id().to_string()]),
        "sending SIGINT",
    );
    let status = hold.wait().expect("wait for hold");
    assert!(status.success(), "hold exited with {status}");
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut rest).unwrap();
    assert!(rest.contains("Outputs were not changed"), "{rest}");

    // Not tripped, output still on: the exit changed nothing by itself.
    let state = sim.json(&["safety", "status"]);
    assert_eq!(state["failsafe"], false, "{state}");
    assert_eq!(sim.json(&["state"])["outputs"], 1);

    // Without a feeder the watchdog then trips, as documented.
    sleep(Duration::from_millis(1500));
    assert_eq!(sim.json(&["safety", "status"])["failsafe"], true);
}
