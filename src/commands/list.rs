use anyhow::Result;

use crate::device;
use crate::output;

pub fn run(json: bool) -> Result<()> {
    let devices = device::enumerate_devices()?;

    if json {
        output::print_json(&devices)?;
        return Ok(());
    }

    if devices.is_empty() {
        println!("No Tachyon devices found.");
        return Ok(());
    }

    let duplicates = device::find_duplicates(&devices);

    let headers = &["#", "UID", "PORT", "VERSION", ""];
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|d| {
            let num_str = match d.device_number {
                Some(n) => n.to_string(),
                None => "-".to_string(),
            };
            let dup_mark = if let Some(n) = d.device_number {
                if duplicates.contains(&n) {
                    "⚠ DUPLICATE".to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
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

    output::print_table(headers, &rows);

    if !duplicates.is_empty() {
        eprintln!(
            "\nWarning: duplicate device numbers detected: {:?}",
            duplicates
        );
        eprintln!("Use 'tachyon devnum auto' to assign unique numbers, or specify --uid/--port.");
    }

    Ok(())
}
