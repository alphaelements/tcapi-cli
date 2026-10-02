//! Parsers for command-line arguments (masks, edge modes, pattern steps,
//! `map set` key/value pairs). Everything wire-related lives in `pochi-sdk`;
//! this module only turns user text into the SDK's typed values.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use pochi_sdk::{EdgeMode, PatternStep, PinMap};

/// Parse an unsigned 32-bit number written as `0x` hex or decimal.
pub fn parse_u32(text: &str) -> Result<u32> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse::<u32>(),
    };
    parsed.with_context(|| {
        format!("invalid number {text:?} (expected decimal or 0x hex, max 32 bits)")
    })
}

/// Parse `rising`, `falling` or `both` (case-insensitive).
pub fn parse_edge_mode(text: &str) -> Result<EdgeMode> {
    match text.to_ascii_lowercase().as_str() {
        "rising" => Ok(EdgeMode::Rising),
        "falling" => Ok(EdgeMode::Falling),
        "both" => Ok(EdgeMode::Both),
        other => bail!("invalid edge mode {other:?} (expected rising, falling or both)"),
    }
}

/// Parse one `pattern add` step `<out_bits>:<hold_ticks>`; the hold is in
/// pattern ticks (200 ns each).
pub fn parse_step(text: &str) -> Result<PatternStep> {
    let (bits, ticks) = text
        .split_once(':')
        .with_context(|| format!("invalid step {text:?} (expected <out_bits>:<hold_ticks>)"))?;
    let step = PatternStep {
        out_bits: parse_u32(bits).with_context(|| format!("invalid out_bits in step {text:?}"))?,
        hold_ticks: ticks
            .trim()
            .parse::<u32>()
            .with_context(|| format!("invalid hold_ticks in step {text:?}"))?,
    };
    step.validate()?;
    Ok(step)
}

/// Keys of the `map set` command (everything but the optional `v=2`).
const MAP_KEYS: [&str; 8] = ["flags", "dir", "pu", "pd", "def", "opol", "ipol", "edge"];

/// Build a [`PinMap`] from `map set` arguments `KEY=VALUE` (`flags`, `dir`,
/// `pu`, `pd`, `def`, `opol`, `ipol`, `edge`; an optional `v=2` is accepted).
/// All eight keys are required exactly once; the map is validated against
/// R-M1 by the SDK before it can be sent.
pub fn parse_map_args(args: &[String]) -> Result<PinMap> {
    let mut values: HashMap<&str, u32> = HashMap::new();
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .with_context(|| format!("invalid map argument {arg:?} (expected KEY=VALUE)"))?;
        if key == "v" {
            if value != "2" {
                bail!("unsupported map version v={value} (only v=2)");
            }
            continue;
        }
        let key = MAP_KEYS.iter().find(|k| **k == key).with_context(|| {
            format!("unknown map key {key:?} (expected {})", MAP_KEYS.join(", "))
        })?;
        let parsed =
            parse_u32(value).with_context(|| format!("invalid value for map key {key:?}"))?;
        if values.insert(key, parsed).is_some() {
            bail!("duplicate map key {key:?}");
        }
    }
    let get = |key: &str| -> Result<u32> {
        values.get(key).copied().with_context(|| {
            format!(
                "missing map key {key:?} (all of {} are required)",
                MAP_KEYS.join(", ")
            )
        })
    };
    let map = PinMap {
        flags: u8::try_from(get("flags")?).context("flags must fit in 8 bits")?,
        direction: get("dir")?,
        pull_up: get("pu")?,
        pull_down: get("pd")?,
        out_default: get("def")?,
        out_polarity: get("opol")?,
        in_polarity: get("ipol")?,
        edge: get("edge")?,
    };
    map.validate()?;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const FULL_MAP: [&str; 8] = [
        "flags=0x00",
        "dir=0x0000000f",
        "pu=0x000000f0",
        "pd=0x0c7fff00",
        "def=0x00000005",
        "opol=0x00000008",
        "ipol=0x00000100",
        "edge=0x0c7ffff0",
    ];

    #[test]
    fn u32_accepts_hex_and_decimal() {
        assert_eq!(parse_u32("0x0F").unwrap(), 15);
        assert_eq!(parse_u32("0XFF").unwrap(), 255);
        assert_eq!(parse_u32("42").unwrap(), 42);
        assert_eq!(parse_u32(" 7 ").unwrap(), 7);
        assert_eq!(parse_u32("0xffffffff").unwrap(), u32::MAX);
    }

    #[test]
    fn u32_rejects_malformed_and_overflow() {
        assert!(parse_u32("").is_err());
        assert!(parse_u32("0x").is_err());
        assert!(parse_u32("0x1ffffffff").is_err());
        assert!(parse_u32("-1").is_err());
        assert!(parse_u32("zz").is_err());
    }

    #[test]
    fn edge_mode_parses_case_insensitively() {
        assert_eq!(parse_edge_mode("rising").unwrap(), EdgeMode::Rising);
        assert_eq!(parse_edge_mode("Falling").unwrap(), EdgeMode::Falling);
        assert_eq!(parse_edge_mode("BOTH").unwrap(), EdgeMode::Both);
        assert!(parse_edge_mode("up").is_err());
    }

    #[test]
    fn step_parses_bits_and_ticks() {
        let step = parse_step("0x5:100").unwrap();
        assert_eq!(step.out_bits, 5);
        assert_eq!(step.hold_ticks, 100);
        assert_eq!(parse_step("3:1").unwrap().out_bits, 3);
    }

    #[test]
    fn step_rejects_malformed_input() {
        assert!(parse_step("5").is_err(), "missing colon");
        assert!(parse_step("0x5:").is_err(), "empty ticks");
        assert!(parse_step(":5").is_err(), "empty bits");
        assert!(parse_step("0x5:0").is_err(), "zero hold");
        assert!(parse_step("0x5:-1").is_err(), "negative hold");
        assert!(parse_step("0x5:1.5").is_err(), "fractional hold");
        assert!(parse_step("0x800000:1").is_err(), "GP23 is not a DIO pin");
    }

    #[test]
    fn map_args_build_the_full_map() {
        let map = parse_map_args(&args(&FULL_MAP)).unwrap();
        assert_eq!(map.direction, 0x0f);
        assert_eq!(map.pull_up, 0xf0);
        assert_eq!(map.pull_down, 0x0c7f_ff00);
        assert_eq!(map.out_default, 5);
        assert_eq!(map.out_polarity, 8);
        assert_eq!(map.in_polarity, 0x100);
        assert_eq!(map.edge, 0x0c7f_fff0);
    }

    #[test]
    fn map_args_accept_any_order_and_explicit_version() {
        let mut reversed = args(&FULL_MAP);
        reversed.reverse();
        reversed.push("v=2".to_string());
        assert_eq!(
            parse_map_args(&reversed).unwrap(),
            parse_map_args(&args(&FULL_MAP)).unwrap()
        );
    }

    #[test]
    fn map_args_reject_missing_key() {
        let partial = args(&FULL_MAP[..7]);
        assert!(parse_map_args(&partial).is_err());
    }

    #[test]
    fn map_args_reject_unknown_key_and_bare_word() {
        let mut unknown = args(&FULL_MAP);
        unknown.push("bogus=1".to_string());
        assert!(parse_map_args(&unknown).is_err());
        assert!(parse_map_args(&args(&["dir"])).is_err());
    }

    #[test]
    fn map_args_reject_invalid_map() {
        // pull-up and pull-down on the same pin violates R-M1.
        let mut bad = args(&FULL_MAP);
        bad[2] = "pu=0x0c7fff00".to_string();
        assert!(parse_map_args(&bad).is_err());
    }

    #[test]
    fn map_args_reject_duplicate_key_wrong_version_and_wide_flags() {
        let mut duplicate = args(&FULL_MAP);
        duplicate.push("dir=0x1".to_string());
        assert!(parse_map_args(&duplicate).is_err());

        let mut version = args(&FULL_MAP);
        version.push("v=1".to_string());
        assert!(parse_map_args(&version).is_err());

        let mut flags = args(&FULL_MAP);
        flags[0] = "flags=0x100".to_string();
        assert!(parse_map_args(&flags).is_err());
    }
}
