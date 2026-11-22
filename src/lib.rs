//! `tcapi` — command-line tool for Tcapi-DIO (RP2350 high-speed digital I/O).
//!
//! The binary in `main.rs` only parses arguments; every command lives in
//! [`commands`] and talks to the device through `tcapi-sdk`.

pub mod commands;
pub mod device;
pub mod exit;
pub mod output;
pub mod parse;
