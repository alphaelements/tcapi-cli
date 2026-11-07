//! Test support: a fake Pochi-DIO on a PTY whose replies are taken from the
//! golden transcripts in `protocol/golden/`, so no wire text is hand-written.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pochi_sdk::Connection;
use serialport::{SerialPort, TTYPort};

/// One golden test case: its exchanges in order.
struct Case {
    exchanges: Vec<(String, Vec<String>)>,
}

/// A parsed golden transcript file.
pub struct Golden {
    cases: Vec<Case>,
}

impl Golden {
    pub fn load(file: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("protocol/golden")
            .join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read golden {}: {e}", path.display()));

        let mut cases = Vec::new();
        let mut current: Vec<(String, Vec<String>)> = Vec::new();
        let mut flush = |current: &mut Vec<(String, Vec<String>)>| {
            if !current.is_empty() {
                cases.push(Case {
                    exchanges: std::mem::take(current),
                });
            }
        };
        for line in text.lines() {
            if line.trim().is_empty() {
                flush(&mut current);
            } else if let Some(request) = line.strip_prefix("> ") {
                current.push((request.to_string(), Vec::new()));
            } else if let Some(wire) = line.strip_prefix("< ") {
                // `< ! EVENT ...` lines are events, not part of a response;
                // `<   x` is a continuation line whose wire form is `  x`.
                if let Some((_, response)) = current.last_mut() {
                    response.push(wire.to_string());
                }
            }
        }
        flush(&mut current);
        Self { cases }
    }

    /// The response to `request` in the first case that sent every request
    /// of `earlier` before it (in order).
    pub fn reply(&self, earlier: &[&str], request: &str) -> Vec<String> {
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        'cases: for case in &self.cases {
            let mut at = 0;
            for wanted in earlier {
                match case.exchanges[at..]
                    .iter()
                    .position(|(r, _)| same(r, wanted))
                {
                    Some(i) => at += i + 1,
                    None => continue 'cases,
                }
            }
            if let Some((_, response)) = case.exchanges[at..].iter().find(|(r, _)| same(r, request))
            {
                return response.clone();
            }
        }
        panic!("golden has no exchange {earlier:?} -> {request:?}");
    }
}

/// Push event lines (`! ...`) of a golden file, in order, as wire lines.
pub fn golden_events(file: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("protocol/golden")
        .join(file);
    std::fs::read_to_string(path)
        .expect("golden file")
        .lines()
        .filter_map(|l| l.strip_prefix("< ! "))
        .map(|l| format!("! {l}"))
        .collect()
}

/// What the fake device answers.
#[derive(Default)]
pub struct Script {
    exact: HashMap<String, Vec<String>>,
    prefix: Vec<(String, Vec<String>)>,
    /// Lines written right after the reply to the given request.
    trailing: HashMap<String, Vec<String>>,
    /// Close the PTY after answering this request (device unplugged).
    close_after: Option<String>,
}

impl Script {
    /// A device that answers `version`, `caps`, `info` and `ping` from the
    /// golden `version-info.txt`.
    pub fn new() -> Self {
        let golden = Golden::load("version-info.txt");
        let mut script = Self::default();
        for request in ["version", "caps", "info"] {
            script = script.reply(request, golden.reply(&[], request));
        }
        script
    }

    /// Answer exactly `request` with `lines`.
    pub fn reply(mut self, request: &str, lines: Vec<String>) -> Self {
        self.exact.insert(request.to_lowercase(), lines);
        self
    }

    /// Answer every request starting with `prefix` with `lines`.
    pub fn reply_prefix(mut self, prefix: &str, lines: Vec<String>) -> Self {
        self.prefix.push((prefix.to_lowercase(), lines));
        self
    }

    /// Send `lines` right after the reply to `request` (push events).
    pub fn then_send(mut self, request: &str, lines: Vec<String>) -> Self {
        self.trailing.insert(request.to_lowercase(), lines);
        self
    }

    /// Disconnect after answering `request`.
    pub fn disconnect_after(mut self, request: &str) -> Self {
        self.close_after = Some(request.to_lowercase());
        self
    }

    fn answer(&self, request: &str) -> Vec<String> {
        let key = request.trim().to_lowercase();
        if let Some(lines) = self.exact.get(&key) {
            return lines.clone();
        }
        if let Some(n) = key.strip_prefix("ping ") {
            return vec![format!("OK pong={n}")];
        }
        for (prefix, lines) in &self.prefix {
            if key.starts_with(prefix) {
                return lines.clone();
            }
        }
        vec!["ERR UNKNOWN_CMD".to_string()]
    }
}

/// How long the fake device lingers before it hangs up.
const DRAIN_BEFORE_CLOSE: Duration = Duration::from_millis(300);

type Overrides = Arc<Mutex<HashMap<String, Vec<String>>>>;

static PTY_PAIR_LOCK: Mutex<()> = Mutex::new(());

/// A running fake device. Dropping it stops the thread.
///
/// The host side is wrapped with `Connection::from_port`: opening a PTY by
/// path would fail while asserting DTR (a PTY has no modem lines).
pub struct FakeDevice {
    commands: Arc<Mutex<Vec<String>>>,
    /// Replies installed while the device runs; they win over the script.
    overrides: Overrides,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeDevice {
    /// Start the device and connect to it; the connection is checked for
    /// `proto=2` exactly as `Connection::open` does.
    pub fn connect(script: Script) -> (pochi_sdk::Result<Connection>, Self) {
        let (device, slave) = Self::start(script);
        (Connection::from_port(Box::new(slave)), device)
    }

    /// Like [`FakeDevice::connect`] for devices that must accept the
    /// connection.
    pub fn connected(script: Script) -> (Connection, Self) {
        let (conn, device) = Self::connect(script);
        (conn.expect("connect to the fake device"), device)
    }

    fn start(script: Script) -> (Self, TTYPort) {
        // `TTYPort::pair` finds the slave with `ptsname`, which returns a
        // pointer to a static buffer: concurrent calls can hand two tests
        // each other's slave. Serialise the creation.
        let (mut master, slave) = {
            let _guard = PTY_PAIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            TTYPort::pair().expect("PTY pair")
        };
        let commands = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let overrides: Overrides = Arc::new(Mutex::new(HashMap::new()));

        let handle = std::thread::spawn({
            let commands = Arc::clone(&commands);
            let stop = Arc::clone(&stop);
            let overrides = Arc::clone(&overrides);
            move || {
                master
                    .set_timeout(Duration::from_millis(20))
                    .expect("set_timeout");
                let mut pending: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 256];
                while !stop.load(Ordering::Relaxed) {
                    match master.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => pending.extend_from_slice(&chunk[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(_) => break,
                    }
                    while let Some(at) = pending.iter().position(|&b| b == b'\n') {
                        let raw: Vec<u8> = pending.drain(..=at).collect();
                        let request = String::from_utf8_lossy(&raw)
                            .trim_end_matches(['\r', '\n'])
                            .to_string();
                        if request.is_empty() {
                            continue;
                        }
                        commands.lock().expect("commands").push(request.clone());
                        let key = request.trim().to_lowercase();
                        let overridden = overrides.lock().expect("overrides").get(&key).cloned();
                        let mut lines = overridden.unwrap_or_else(|| script.answer(&request));
                        if let Some(extra) = script.trailing.get(&key) {
                            lines.extend(extra.iter().cloned());
                        }
                        for line in lines {
                            master.write_all(format!("{line}\r\n").as_bytes()).ok();
                        }
                        master.flush().ok();
                        if script.close_after.as_deref() == Some(key.as_str()) {
                            // Closing the master discards bytes the host has
                            // not read yet; give it time to read the reply.
                            std::thread::sleep(DRAIN_BEFORE_CLOSE);
                            return;
                        }
                    }
                }
            }
        });

        (
            Self {
                commands,
                overrides,
                stop,
                handle: Some(handle),
            },
            slave,
        )
    }

    /// From now on answer `request` with `lines`, whatever the script says.
    pub fn set_reply(&self, request: &str, lines: Vec<String>) {
        self.overrides
            .lock()
            .expect("overrides")
            .insert(request.to_lowercase(), lines);
    }

    /// Every request line received so far.
    pub fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("commands").clone()
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
