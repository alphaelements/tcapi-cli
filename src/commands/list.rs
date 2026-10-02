use anyhow::Result;
use pochi_sdk::discovery;

use crate::device;
use crate::output;

pub fn run(json: bool) -> Result<()> {
    let devices = device::enumerate_devices()?;

    if json {
        output::print_json(&devices)?;
        return Ok(());
    }

    if devices.is_empty() {
        println!("No Pochi-DIO devices found.");
        return Ok(());
    }

    let duplicates = discovery::find_duplicate_numbers(&devices);

    let headers = &["#", "UID", "PORT", "VERSION", ""];
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|d| {
            let num_str = d
                .device_number
                .map_or_else(|| "-".to_string(), |n| n.to_string());
            let dup_mark = match d.device_number {
                Some(n) if duplicates.contains(&n) => "⚠ DUPLICATE".to_string(),
                _ => String::new(),
            };
            vec![
                num_str,
                d.uid.clone(),
                d.port.clone(),
                d.version.clone().unwrap_or_default(),
                dup_mark,
            ]
        })
        .collect();

    output::print_table(headers, &rows)?;

    if !duplicates.is_empty() {
        eprintln!("\nWarning: duplicate device numbers detected: {duplicates:?}");
        eprintln!("Use 'pochi devnum auto' to assign unique numbers, or specify --uid/--port.");
    }

    Ok(())
}
