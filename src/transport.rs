use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
const BAUD_RATE: u32 = 115200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Ok(String),
    Err(String),
    Push(String),
}

impl Response {
    pub fn ok_data(&self) -> Result<&str> {
        match self {
            Response::Ok(data) => Ok(data.as_str()),
            Response::Err(code) => bail!("device error: {}", error_description(code)),
            Response::Push(_) => bail!("unexpected push event"),
        }
    }
}

fn error_description(code: &str) -> String {
    let hint = match code {
        "BAD_CMD" => "unknown command",
        "BAD_PARAM" => "invalid parameter",
        "BAD_MAP" => "map validation failed (overlapping or out-of-range pins)",
        "BUSY" => "device is busy (stop current operation first)",
        "FLASH" => "flash write/read failed",
        "TOO_LONG" => "payload too long",
        "NOT_ARMED" => "timestamp not armed (run 'ts arm' first)",
        "SYNC_STATE" => "invalid sync state transition",
        "SYNC_TIMEOUT" => "sync pulse not detected (check wiring)",
        "NO_RESOURCE" => {
            "no matching resource (e.g. write to input-only map, or read with no inputs)"
        }
        "NOT_RUNNING" => "pattern is not running (stop only applies while a pattern is running)",
        "UNKNOWN_CMD" => "unknown text command",
        "LINE_TOO_LONG" => "command line exceeds 128 bytes",
        _ => return format!("ERR {code}"),
    };
    format!("ERR {code}: {hint}")
}

pub fn parse_response(line: &str) -> Result<Response> {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix("OK") {
        let data = rest.strip_prefix(' ').unwrap_or("").to_string();
        Ok(Response::Ok(data))
    } else if let Some(rest) = line.strip_prefix("ERR ") {
        Ok(Response::Err(rest.to_string()))
    } else if let Some(rest) = line.strip_prefix("! ") {
        Ok(Response::Push(rest.to_string()))
    } else {
        bail!("unexpected response: {line}")
    }
}

pub fn open_port(path: &str) -> Result<Box<dyn serialport::SerialPort>> {
    serialport::new(path, BAUD_RATE)
        .timeout(DEFAULT_TIMEOUT)
        .open()
        .with_context(|| format!("failed to open serial port: {path}"))
}

pub fn send_command(port: &mut Box<dyn serialport::SerialPort>, cmd: &str) -> Result<Response> {
    let cmd_line = format!("{cmd}\r\n");
    port.write_all(cmd_line.as_bytes())
        .context("failed to write command")?;
    port.flush().context("failed to flush")?;

    let mut reader = BufReader::new(port.try_clone().context("failed to clone port")?);
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read response")?;
        if line.is_empty() {
            bail!("device disconnected");
        }
        let resp = parse_response(&line)?;
        match &resp {
            Response::Push(_) => continue,
            _ => return Ok(resp),
        }
    }
}

pub fn send_command_raw(
    port: &mut Box<dyn serialport::SerialPort>,
    cmd: &str,
) -> Result<Vec<String>> {
    let cmd_line = format!("{cmd}\r\n");
    port.write_all(cmd_line.as_bytes())
        .context("failed to write command")?;
    port.flush().context("failed to flush")?;

    let mut reader = BufReader::new(port.try_clone().context("failed to clone port")?);
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read response")?;
        if line.is_empty() {
            bail!("device disconnected");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        lines.push(trimmed.clone());
        if trimmed.starts_with("OK") || trimmed.starts_with("ERR ") {
            return Ok(lines);
        }
    }
}

/// Send a command whose response is `OK count=N` followed by N detail
/// lines (e.g. `ts get`). Returns all lines including the OK line.
pub fn send_command_counted(
    port: &mut Box<dyn serialport::SerialPort>,
    cmd: &str,
) -> Result<Vec<String>> {
    let cmd_line = format!("{cmd}\r\n");
    port.write_all(cmd_line.as_bytes())
        .context("failed to write command")?;
    port.flush().context("failed to flush")?;

    let mut reader = BufReader::new(port.try_clone().context("failed to clone port")?);
    let mut lines = Vec::new();
    let mut remaining: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read response")?;
        if line.is_empty() {
            bail!("device disconnected");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        lines.push(trimmed.clone());

        match remaining {
            None => {
                if trimmed.starts_with("ERR ") {
                    return Ok(lines);
                }
                if trimmed.starts_with("OK") {
                    let count = parse_count_field(&trimmed).unwrap_or(0);
                    if count == 0 {
                        return Ok(lines);
                    }
                    remaining = Some(count);
                }
            }
            Some(n) => {
                if n <= 1 {
                    return Ok(lines);
                }
                remaining = Some(n - 1);
            }
        }
    }
}

fn parse_count_field(ok_line: &str) -> Option<usize> {
    ok_line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("count="))
        .and_then(|v| v.parse().ok())
}

/// Number of `chN: OUT`/`chN: IN` detail lines that follow a v2 `map get`
/// header (ch0..ch24). See design doc §2.4 and §9 R-06.
const MAP_V2_CHANNEL_LINES: usize = 25;

/// Parses the `v=` field out of a `map get` OK header line's data payload.
/// Returns `None` when absent (treated as v1 for backward compat, matching
/// `commands::map::parse_map_response`'s default).
fn parse_map_version_field(ok_data: &str) -> Option<u8> {
    ok_data
        .split_whitespace()
        .find_map(|part| part.strip_prefix("v="))
        .and_then(|v| v.parse().ok())
}

/// Sends `map get` and reads its response using a single `BufReader`.
///
/// The v2 firmware embeds 25 `chN: OUT`/`chN: IN` detail lines after the
/// `OK v=2 ...` header, in the *same* response burst. A fresh `BufReader`
/// per read (as `send_command` does) would risk losing buffered bytes
/// between the header read and the detail-line reads, so this function
/// reads header + detail lines from one `BufReader` (design doc §9 R-02+R-03).
///
/// Returns `(header_response, channel_lines)`. `channel_lines` is `Some`
/// (with exactly [`MAP_V2_CHANNEL_LINES`] entries) only when the header
/// reports `v=2`; it is `None` for v1 responses or error responses.
pub fn send_command_map_get(
    port: &mut Box<dyn serialport::SerialPort>,
) -> Result<(Response, Option<Vec<String>>)> {
    let cmd_line = "map get\r\n";
    port.write_all(cmd_line.as_bytes())
        .context("failed to write command")?;
    port.flush().context("failed to flush")?;

    let mut reader = BufReader::new(port.try_clone().context("failed to clone port")?);

    let header = loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read response")?;
        if line.is_empty() {
            bail!("device disconnected");
        }
        let resp = parse_response(&line)?;
        match resp {
            Response::Push(_) => continue,
            other => break other,
        }
    };

    let is_v2 = matches!(&header, Response::Ok(data) if parse_map_version_field(data) == Some(2));
    if !is_v2 {
        return Ok((header, None));
    }

    let mut channel_lines = Vec::with_capacity(MAP_V2_CHANNEL_LINES);
    for _ in 0..MAP_V2_CHANNEL_LINES {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("failed to read map channel line")?;
        if line.is_empty() {
            bail!("device disconnected while reading map channel lines");
        }
        channel_lines.push(line.trim_end_matches(['\r', '\n']).to_string());
    }

    Ok((header, Some(channel_lines)))
}

pub fn read_push_events(
    port: &mut Box<dyn serialport::SerialPort>,
) -> Result<impl Iterator<Item = Result<String>>> {
    let reader = BufReader::new(port.try_clone().context("failed to clone port")?);
    Ok(reader.lines().map(|line| {
        let line = line.context("failed to read line")?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = trimmed.strip_prefix("! ") {
            Ok(rest.to_string())
        } else {
            bail!("unexpected line in push stream: {trimmed}")
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_empty() {
        let resp = parse_response("OK\r\n").unwrap();
        assert_eq!(resp, Response::Ok(String::new()));
    }

    #[test]
    fn parse_ok_with_data() {
        let resp = parse_response("OK v0.1.0 proto=1\r\n").unwrap();
        assert_eq!(resp, Response::Ok("v0.1.0 proto=1".to_string()));
    }

    #[test]
    fn parse_err() {
        let resp = parse_response("ERR BAD_CMD\r\n").unwrap();
        assert_eq!(resp, Response::Err("BAD_CMD".to_string()));
    }

    #[test]
    fn map_version_field_v2() {
        assert_eq!(
            parse_map_version_field("v=2 out=4 in=21 flags=0x01"),
            Some(2)
        );
    }

    #[test]
    fn map_version_field_v1() {
        assert_eq!(
            parse_map_version_field("v=1 out=0:12 in=12:11 pull=up flags=0x01"),
            Some(1)
        );
    }

    #[test]
    fn map_version_field_missing() {
        assert_eq!(parse_map_version_field("out=4 in=21"), None);
    }

    #[test]
    fn parse_push() {
        let resp = parse_response("! INPUT 0x000005 t=1234567890\r\n").unwrap();
        assert_eq!(
            resp,
            Response::Push("INPUT 0x000005 t=1234567890".to_string())
        );
    }

    #[test]
    fn parse_unexpected() {
        assert!(parse_response("garbage\r\n").is_err());
    }

    #[test]
    fn ok_data_success() {
        let resp = Response::Ok("hello".to_string());
        assert_eq!(resp.ok_data().unwrap(), "hello");
    }

    #[test]
    fn ok_data_err() {
        let resp = Response::Err("BAD_CMD".to_string());
        assert!(resp.ok_data().is_err());
    }

    #[test]
    fn err_not_running_has_hint() {
        let resp = Response::Err("NOT_RUNNING".to_string());
        let err = resp.ok_data().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("NOT_RUNNING"));
        assert!(msg.contains("not running"));
    }

    #[test]
    fn err_unknown_code_falls_back_to_raw() {
        let resp = Response::Err("WAT_IS_THIS".to_string());
        let err = resp.ok_data().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WAT_IS_THIS"));
    }
}
