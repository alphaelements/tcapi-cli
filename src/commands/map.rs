use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use tcapi_sdk::{ch_to_gp, config, Connection, PinMap, DIO_CHANNELS};

use crate::device::{self, DeviceSelector};
use crate::output;
use crate::parse::parse_map_args;

/// The eight device map fields, in `map get` order, as `key  mask  meaning`.
pub fn mask_lines(map: &PinMap) -> Vec<String> {
    vec![
        format!("flags: {:#04x}  (reserved)", map.flags),
        format!("dir:   {:#010x}  (1 = output)", map.direction),
        format!("pu:    {:#010x}  (input pull-up)", map.pull_up),
        format!("pd:    {:#010x}  (input pull-down)", map.pull_down),
        format!("def:   {:#010x}  (output initial level)", map.out_default),
        format!("opol:  {:#010x}  (output inverted)", map.out_polarity),
        format!("ipol:  {:#010x}  (input inverted)", map.in_polarity),
        format!("edge:  {:#010x}  (input edge events)", map.edge),
    ]
}

/// Table headers of [`channel_rows`].
pub const CHANNEL_HEADERS: [&str; 7] = ["CH", "GP", "DIR", "PULL", "DEFAULT", "INVERT", "EDGE"];

/// One row per logical channel `ch0..=ch24` with its physical GP pin and the
/// settings that apply to its direction (`-` where a setting does not apply).
pub fn channel_rows(map: &PinMap) -> Result<Vec<Vec<String>>> {
    (0..DIO_CHANNELS)
        .map(|ch| {
            let gp = ch_to_gp(ch)?;
            let bit = 1u32 << gp;
            let set = |mask: u32| mask & bit != 0;
            let is_output = set(map.direction);
            let yes_no = |on: bool| if on { "yes" } else { "no" }.to_string();
            let na = || "-".to_string();
            Ok(if is_output {
                vec![
                    format!("ch{ch}"),
                    format!("GP{gp}"),
                    "OUT".to_string(),
                    na(),
                    if set(map.out_default) { "1" } else { "0" }.to_string(),
                    yes_no(set(map.out_polarity)),
                    na(),
                ]
            } else {
                let pull = match (set(map.pull_up), set(map.pull_down)) {
                    (true, _) => "up",
                    (_, true) => "down",
                    _ => "none",
                };
                vec![
                    format!("ch{ch}"),
                    format!("GP{gp}"),
                    "IN".to_string(),
                    pull.to_string(),
                    na(),
                    yes_no(set(map.in_polarity)),
                    yes_no(set(map.edge)),
                ]
            })
        })
        .collect()
}

pub fn run_show(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    show(&mut conn, json, &mut io::stdout().lock())
}

pub fn show(conn: &mut Connection, json: bool, out: &mut impl Write) -> Result<()> {
    let map = conn.map_get()?;

    if json {
        output::write_json(out, &map)?;
    } else {
        writeln!(out, "Pin map (v2)")?;
        for line in mask_lines(&map) {
            writeln!(out, "  {line}")?;
        }
        writeln!(out)?;
        output::write_table(out, &CHANNEL_HEADERS, &channel_rows(&map)?)?;
    }
    Ok(())
}

pub fn run_set(selector: &DeviceSelector, args: &[String], json: bool) -> Result<()> {
    let map = parse_map_args(args)?;
    let (_info, mut conn) = device::open_device(selector)?;
    apply(&mut conn, &map, "Map set.", json, &mut io::stdout().lock())
}

/// Send `map` to the device and report `message` (human) or the map (JSON).
pub fn apply(
    conn: &mut Connection,
    map: &PinMap,
    message: &str,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    conn.map_set(map)?;

    if json {
        output::write_json(out, &serde_json::json!({"status": "ok", "map": map}))?;
    } else {
        writeln!(out, "{message}")?;
    }
    Ok(())
}

pub fn run_save(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.map_save()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Map saved to flash.");
    }
    Ok(())
}

pub fn run_default(selector: &DeviceSelector, json: bool) -> Result<()> {
    let (_info, mut conn) = device::open_device(selector)?;
    conn.map_default()?;

    if json {
        output::print_json(&serde_json::json!({"status": "ok"}))?;
    } else {
        println!("Default map loaded (all pins input, pull-down).");
    }
    Ok(())
}

/// Read and parse a `map.toml` with the SDK parser; errors name the file and
/// line.
pub fn load_map_file(file: &str) -> Result<PinMap> {
    let src = std::fs::read_to_string(Path::new(file))
        .with_context(|| format!("failed to read map file: {file}"))?;
    Ok(config::parse_map(&src, file)?)
}

pub fn run_load(selector: &DeviceSelector, file: &str, json: bool) -> Result<()> {
    let map = load_map_file(file)?;
    let (_info, mut conn) = device::open_device(selector)?;
    apply(
        &mut conn,
        &map,
        &format!("Map loaded from {file}."),
        json,
        &mut io::stdout().lock(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tcapi_sdk::VALID_DIO_MASK;

    fn sample_map() -> PinMap {
        PinMap {
            flags: 0,
            direction: 0x0000_000f | (1 << 26),
            pull_up: 0x0000_00f0,
            pull_down: VALID_DIO_MASK & !0x0400_00ff,
            out_default: 0x0000_0005,
            out_polarity: 0x0000_0008,
            in_polarity: 0x0000_0100,
            edge: 0x0000_00f0,
        }
    }

    #[test]
    fn mask_lines_show_all_eight_keys_with_hex_masks() {
        let map = sample_map();
        let lines = mask_lines(&map);
        assert_eq!(lines.len(), 8);
        for key in [
            "flags:", "dir:", "pu:", "pd:", "def:", "opol:", "ipol:", "edge:",
        ] {
            assert!(lines.iter().any(|l| l.starts_with(key)), "missing {key}");
        }
        assert!(lines[1].contains("0x0400000f"));
        assert!(lines[2].contains("0x000000f0"));
        assert!(lines[4].contains("0x00000005"));
        assert!(lines[5].contains("0x00000008"));
        assert!(lines[6].contains("0x00000100"));
        assert!(lines[7].contains("0x000000f0"));
        assert!(lines[0].contains("0x00"));
    }

    #[test]
    fn channel_rows_cover_ch0_to_ch24_with_gp_numbers() {
        let rows = channel_rows(&sample_map()).unwrap();
        assert_eq!(rows.len(), 25);
        assert_eq!(rows[0][..2], ["ch0", "GP0"]);
        assert_eq!(rows[22][..2], ["ch22", "GP22"]);
        assert_eq!(rows[23][..2], ["ch23", "GP26"]);
        assert_eq!(rows[24][..2], ["ch24", "GP27"]);
        assert!(rows.iter().all(|r| r.len() == CHANNEL_HEADERS.len()));
    }

    #[test]
    fn channel_rows_describe_outputs() {
        let rows = channel_rows(&sample_map()).unwrap();
        // ch0: output, default 1, not inverted
        assert_eq!(rows[0], ["ch0", "GP0", "OUT", "-", "1", "no", "-"]);
        // ch3: output, default 0, inverted
        assert_eq!(rows[3], ["ch3", "GP3", "OUT", "-", "0", "yes", "-"]);
        // ch23 = GP26 is an output too
        assert_eq!(rows[23][2], "OUT");
    }

    #[test]
    fn channel_rows_describe_inputs() {
        let rows = channel_rows(&sample_map()).unwrap();
        // ch4: input, pull-up, edge on
        assert_eq!(rows[4], ["ch4", "GP4", "IN", "up", "-", "no", "yes"]);
        // ch8: input, pull-down, inverted, no edge
        assert_eq!(rows[8], ["ch8", "GP8", "IN", "down", "-", "yes", "no"]);
        // ch24 = GP27: input with pull-down and no edge in the sample map
        assert_eq!(rows[24][..4], ["ch24", "GP27", "IN", "down"]);
    }

    #[test]
    fn channel_rows_report_no_pull() {
        let map = PinMap {
            pull_down: 0,
            edge: 0,
            ..PinMap::default()
        };
        let rows = channel_rows(&map).unwrap();
        assert_eq!(rows[0][3], "none");
    }

    #[test]
    fn default_map_is_all_input_pull_down_edge() {
        let rows = channel_rows(&PinMap::default()).unwrap();
        assert!(rows
            .iter()
            .all(|r| r[2] == "IN" && r[3] == "down" && r[6] == "yes"));
    }

    #[test]
    fn load_map_file_reports_missing_file() {
        let err = load_map_file("/nonexistent/tcapi-map.toml").unwrap_err();
        assert!(err.to_string().contains("failed to read map file"));
    }

    #[test]
    fn load_map_file_reports_file_and_line_for_invalid_toml() {
        let dir = std::env::temp_dir().join(format!("tcapi-cli-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v1.toml");
        std::fs::write(&path, "version = 1\n").unwrap();
        let err = load_map_file(path.to_str().unwrap()).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("v1.toml:1:"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
