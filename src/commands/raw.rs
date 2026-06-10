use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};

use crate::device::{self, DeviceSelector};
use crate::transport;

pub fn run_raw(selector: &DeviceSelector, text: &str, json: bool) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    let lines = transport::send_command_raw(&mut port, text)?;

    if json {
        crate::output::print_json(&serde_json::json!({"lines": lines}))?;
    } else {
        for line in &lines {
            println!("{line}");
        }
    }
    Ok(())
}

pub fn run_interactive(selector: &DeviceSelector) -> Result<()> {
    let (_info, mut port) = device::open_device(selector)?;
    println!("Pochi-DIO interactive mode (type 'exit' or Ctrl+D to quit)");

    let stdin = io::stdin();
    loop {
        print!("pochi> ");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        let n = stdin
            .lock()
            .read_line(&mut input)
            .context("failed to read input")?;
        if n == 0 {
            println!();
            break;
        }

        let cmd = input.trim();
        if cmd.is_empty() {
            continue;
        }
        if cmd == "exit" || cmd == "quit" {
            break;
        }

        match transport::send_command_raw(&mut port, cmd) {
            Ok(lines) => {
                for line in &lines {
                    println!("{line}");
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
            }
        }
    }

    Ok(())
}
