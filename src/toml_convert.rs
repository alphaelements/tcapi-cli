use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct MapConfig {
    #[allow(dead_code)]
    version: Option<u8>,
    output: Option<MapOutputConfig>,
    input: Option<MapInputConfig>,
}

#[derive(Debug, Deserialize)]
struct MapOutputConfig {
    base: Option<u8>,
    count: Option<u8>,
    default: Option<u32>,
    polarity: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MapInputConfig {
    base: Option<u8>,
    count: Option<u8>,
    pull: Option<String>,
    timestamp: Option<bool>,
}

pub fn map_toml_to_hex(toml_str: &str) -> Result<String> {
    let config: MapConfig = toml::from_str(toml_str).context("failed to parse map TOML")?;

    let mut buf = [0u8; 24];
    buf[0] = 1; // version

    if let Some(out) = &config.output {
        buf[1] = out.base.unwrap_or(0);
        buf[2] = out.count.unwrap_or(0);
        let default = out.default.unwrap_or(0);
        buf[8..12].copy_from_slice(&default.to_le_bytes());
        let polarity = out.polarity.unwrap_or(0);
        buf[12..16].copy_from_slice(&polarity.to_le_bytes());
    }

    if let Some(inp) = &config.input {
        buf[3] = inp.base.unwrap_or(0);
        buf[4] = inp.count.unwrap_or(0);
        let pull = match inp.pull.as_deref() {
            Some("none") => 0,
            Some("up") => 1,
            Some("down") | None => 2,
            Some(other) => bail!("invalid pull value: {other}"),
        };
        buf[5] = pull;

        let mut flags = 0u8;
        if inp.timestamp.unwrap_or(false) {
            flags |= 0x01;
        }
        buf[6] = flags;
    }

    Ok(hex::encode(buf))
}

#[derive(Debug, Deserialize)]
struct PatternConfig {
    #[allow(dead_code)]
    version: Option<u8>,
    #[allow(dead_code)]
    unit: Option<String>,
    steps: Option<Vec<PatternStep>>,
    edge: Option<Vec<EdgeEvent>>,
}

#[derive(Debug, Deserialize)]
struct PatternStep {
    out: u32,
    hold: u32,
}

#[derive(Debug, Deserialize)]
struct EdgeEvent {
    ch: u8,
    t: u64,
    level: u8,
}

pub fn pattern_toml_to_hex(toml_str: &str) -> Result<String> {
    let config: PatternConfig = toml::from_str(toml_str).context("failed to parse pattern TOML")?;

    let steps = if let Some(steps) = config.steps {
        steps
    } else if let Some(edges) = config.edge {
        edges_to_steps(&edges)?
    } else {
        bail!("pattern TOML must contain 'steps' or 'edge'");
    };

    if steps.is_empty() {
        bail!("pattern must have at least one step");
    }
    if steps.len() > 256 {
        bail!("pattern exceeds maximum 256 steps");
    }

    let step_count = steps.len() as u16;
    let mut data = Vec::with_capacity(2 + steps.len() * 8);
    data.extend_from_slice(&step_count.to_le_bytes());
    for step in &steps {
        data.extend_from_slice(&step.out.to_le_bytes());
        data.extend_from_slice(&step.hold.to_le_bytes());
    }

    Ok(hex::encode(data))
}

fn edges_to_steps(edges: &[EdgeEvent]) -> Result<Vec<PatternStep>> {
    if edges.is_empty() {
        bail!("edge list is empty");
    }

    let mut sorted: Vec<&EdgeEvent> = edges.iter().collect();
    sorted.sort_by_key(|e| e.t);

    let mut timepoints: Vec<u64> = sorted.iter().map(|e| e.t).collect();
    timepoints.dedup();

    let mut steps = Vec::new();
    let mut current_bits: u32 = 0;

    for (i, &t) in timepoints.iter().enumerate() {
        for edge in &sorted {
            if edge.t == t {
                if edge.level != 0 {
                    current_bits |= 1 << edge.ch;
                } else {
                    current_bits &= !(1 << edge.ch);
                }
            }
        }

        let hold = if i + 1 < timepoints.len() {
            let diff = timepoints[i + 1] - t;
            diff as u32
        } else {
            0
        };

        if hold > 0 || i + 1 == timepoints.len() {
            steps.push(PatternStep {
                out: current_bits,
                hold,
            });
        }
    }

    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_all_input() {
        let toml = r#"
version = 1
[input]
base = 0
count = 23
pull = "down"
timestamp = true
"#;
        let hex = map_toml_to_hex(toml).unwrap();
        assert_eq!(hex.len(), 48); // 24 bytes = 48 hex chars
        let bytes = hex::decode(&hex).unwrap();
        assert_eq!(bytes[0], 1); // version
        assert_eq!(bytes[1], 0); // out_base
        assert_eq!(bytes[2], 0); // out_count
        assert_eq!(bytes[3], 0); // in_base
        assert_eq!(bytes[4], 23); // in_count
        assert_eq!(bytes[5], 2); // pull = down
        assert_eq!(bytes[6], 1); // flags = timestamp
    }

    #[test]
    fn map_output_12_input_11() {
        let toml = r#"
version = 1
[output]
base = 0
count = 12
default = 0
polarity = 0

[input]
base = 12
count = 11
pull = "up"
timestamp = true
"#;
        let hex = map_toml_to_hex(toml).unwrap();
        let bytes = hex::decode(&hex).unwrap();
        assert_eq!(bytes[1], 0); // out_base
        assert_eq!(bytes[2], 12); // out_count
        assert_eq!(bytes[3], 12); // in_base
        assert_eq!(bytes[4], 11); // in_count
        assert_eq!(bytes[5], 1); // pull = up
    }

    #[test]
    fn map_output_only() {
        let toml = r#"
version = 1
[output]
base = 0
count = 23
"#;
        let hex = map_toml_to_hex(toml).unwrap();
        let bytes = hex::decode(&hex).unwrap();
        assert_eq!(bytes[2], 23); // out_count
        assert_eq!(bytes[4], 0); // in_count
    }

    #[test]
    fn pattern_steps() {
        let toml = r#"
version = 1
unit = "us"
steps = [
  { out = 1, hold = 100 },
  { out = 2, hold = 100 },
  { out = 0, hold = 200 },
]
"#;
        let hex = pattern_toml_to_hex(toml).unwrap();
        let bytes = hex::decode(&hex).unwrap();
        let count = u16::from_le_bytes([bytes[0], bytes[1]]);
        assert_eq!(count, 3);
        // first step: out=1, hold=100
        let out = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
        let hold = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
        assert_eq!(out, 1);
        assert_eq!(hold, 100);
    }

    #[test]
    fn pattern_edges() {
        let toml = r#"
version = 1
unit = "us"
[[edge]]
ch = 0
t = 0
level = 1

[[edge]]
ch = 0
t = 100
level = 0

[[edge]]
ch = 1
t = 50
level = 1

[[edge]]
ch = 1
t = 150
level = 0
"#;
        let hex = pattern_toml_to_hex(toml).unwrap();
        let bytes = hex::decode(&hex).unwrap();
        let count = u16::from_le_bytes([bytes[0], bytes[1]]);
        assert!(count >= 3); // at least 3 time points: 0, 50, 100, 150
    }

    #[test]
    fn edges_to_steps_basic() {
        let edges = vec![
            EdgeEvent {
                ch: 0,
                t: 0,
                level: 1,
            },
            EdgeEvent {
                ch: 0,
                t: 100,
                level: 0,
            },
            EdgeEvent {
                ch: 1,
                t: 50,
                level: 1,
            },
            EdgeEvent {
                ch: 1,
                t: 150,
                level: 0,
            },
        ];
        let steps = edges_to_steps(&edges).unwrap();
        assert_eq!(steps[0].out, 0b01); // t=0: ch0=1
        assert_eq!(steps[0].hold, 50); // 50µs until t=50
        assert_eq!(steps[1].out, 0b11); // t=50: ch0=1, ch1=1
        assert_eq!(steps[1].hold, 50); // 50µs until t=100
        assert_eq!(steps[2].out, 0b10); // t=100: ch0=0, ch1=1
        assert_eq!(steps[2].hold, 50); // 50µs until t=150
        assert_eq!(steps[3].out, 0b00); // t=150: all off
    }
}
