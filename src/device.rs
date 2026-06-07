use anyhow::{bail, Context, Result};
use serialport::SerialPortType;

use crate::transport;

const TACHYON_VID: u16 = 0x2E8A;
const TACHYON_PID: u16 = 0x000A;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub uid: String,
    pub device_number: Option<u8>,
    pub port: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeviceSelector {
    pub uid: Option<String>,
    pub device: Option<u8>,
    pub port: Option<String>,
}

pub fn enumerate_ports() -> Result<Vec<String>> {
    let ports = serialport::available_ports().context("failed to list serial ports")?;
    let mut tachyon_ports = Vec::new();
    for port in ports {
        if let SerialPortType::UsbPort(info) = &port.port_type {
            if info.vid == TACHYON_VID
                && info.pid == TACHYON_PID
                && info
                    .product
                    .as_ref()
                    .is_some_and(|p| p.starts_with("Tachyon"))
            {
                tachyon_ports.push(port.port_name);
            }
        }
    }
    Ok(tachyon_ports)
}

pub fn enumerate_devices() -> Result<Vec<DeviceInfo>> {
    let ports = enumerate_ports()?;
    let mut devices = Vec::new();
    for port_path in ports {
        match probe_device(&port_path) {
            Ok(info) => devices.push(info),
            Err(e) => {
                eprintln!("warning: failed to probe {port_path}: {e}");
            }
        }
    }
    Ok(devices)
}

fn probe_device(port_path: &str) -> Result<DeviceInfo> {
    let mut port = transport::open_port(port_path)?;
    let resp = transport::send_command(&mut port, "info")?;
    let data = resp.ok_data()?;
    let (uid, device_number) = parse_info_response(data)?;

    let version = match transport::send_command(&mut port, "version") {
        Ok(resp) => resp
            .ok_data()
            .ok()
            .map(|d| d.split_whitespace().next().unwrap_or(d).to_string()),
        Err(_) => None,
    };

    Ok(DeviceInfo {
        uid,
        device_number,
        port: port_path.to_string(),
        version,
    })
}

fn parse_info_response(data: &str) -> Result<(String, Option<u8>)> {
    let mut uid = String::new();
    let mut devnum: Option<u8> = None;

    for part in data.split_whitespace() {
        if let Some(val) = part.strip_prefix("uid=") {
            uid = val.to_string();
        } else if let Some(val) = part.strip_prefix("devnum=") {
            let n: u16 = val.parse().context("invalid devnum")?;
            devnum = if n == 255 { None } else { Some(n as u8) };
        }
    }

    if uid.is_empty() {
        bail!("missing uid in info response");
    }

    Ok((uid, devnum))
}

pub fn find_duplicates(devices: &[DeviceInfo]) -> Vec<u8> {
    let mut seen = std::collections::HashMap::new();
    let mut duplicates = Vec::new();
    for dev in devices {
        if let Some(num) = dev.device_number {
            let count = seen.entry(num).or_insert(0u32);
            *count += 1;
            if *count == 2 {
                duplicates.push(num);
            }
        }
    }
    duplicates
}

pub fn select_device(devices: &[DeviceInfo], selector: &DeviceSelector) -> Result<DeviceInfo> {
    if let Some(port) = &selector.port {
        return devices
            .iter()
            .find(|d| d.port == *port)
            .cloned()
            .with_context(|| format!("no Tachyon device on port {port}"));
    }

    if let Some(uid) = &selector.uid {
        let uid_upper = uid.to_uppercase();
        return devices
            .iter()
            .find(|d| d.uid.to_uppercase() == uid_upper)
            .cloned()
            .with_context(|| format!("no Tachyon device with UID {uid}"));
    }

    if let Some(num) = selector.device {
        let matches: Vec<_> = devices
            .iter()
            .filter(|d| d.device_number == Some(num))
            .collect();
        return match matches.len() {
            0 => bail!("no Tachyon device with device number {num}"),
            1 => Ok(matches[0].clone()),
            _ => bail!(
                "multiple devices with device number {num}; use --uid or --port to disambiguate"
            ),
        };
    }

    match devices.len() {
        0 => bail!("no Tachyon device found"),
        1 => Ok(devices[0].clone()),
        n => bail!("{n} Tachyon devices found; specify --uid, --device, or --port"),
    }
}

pub fn open_device(
    selector: &DeviceSelector,
) -> Result<(DeviceInfo, Box<dyn serialport::SerialPort>)> {
    if let Some(port_path) = &selector.port {
        let port = transport::open_port(port_path)?;
        let info = DeviceInfo {
            uid: String::new(),
            device_number: None,
            port: port_path.clone(),
            version: None,
        };
        return Ok((info, port));
    }

    let devices = enumerate_devices()?;
    let selected = select_device(&devices, selector)?;
    let port = transport::open_port(&selected.port)?;
    Ok((selected, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(uid: &str, devnum: Option<u8>, port: &str) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_string(),
            device_number: devnum,
            port: port.to_string(),
            version: Some("v0.1.0".to_string()),
        }
    }

    #[test]
    fn select_single_device() {
        let devices = vec![make_device("AABB", Some(1), "/dev/ttyACM0")];
        let sel = DeviceSelector {
            uid: None,
            device: None,
            port: None,
        };
        let result = select_device(&devices, &sel).unwrap();
        assert_eq!(result.uid, "AABB");
    }

    #[test]
    fn select_by_uid() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(2), "/dev/ttyACM1"),
        ];
        let sel = DeviceSelector {
            uid: Some("ccdd".to_string()),
            device: None,
            port: None,
        };
        let result = select_device(&devices, &sel).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_by_device_number() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(2), "/dev/ttyACM1"),
        ];
        let sel = DeviceSelector {
            uid: None,
            device: Some(2),
            port: None,
        };
        let result = select_device(&devices, &sel).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_by_port() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(2), "/dev/ttyACM1"),
        ];
        let sel = DeviceSelector {
            uid: None,
            device: None,
            port: Some("/dev/ttyACM1".to_string()),
        };
        let result = select_device(&devices, &sel).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_multiple_no_selector_fails() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(2), "/dev/ttyACM1"),
        ];
        let sel = DeviceSelector {
            uid: None,
            device: None,
            port: None,
        };
        assert!(select_device(&devices, &sel).is_err());
    }

    #[test]
    fn select_duplicate_devnum_fails() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(1), "/dev/ttyACM1"),
        ];
        let sel = DeviceSelector {
            uid: None,
            device: Some(1),
            port: None,
        };
        assert!(select_device(&devices, &sel).is_err());
    }

    #[test]
    fn find_duplicates_none() {
        let devices = vec![
            make_device("A", Some(1), "a"),
            make_device("B", Some(2), "b"),
        ];
        assert!(find_duplicates(&devices).is_empty());
    }

    #[test]
    fn find_duplicates_present() {
        let devices = vec![
            make_device("A", Some(1), "a"),
            make_device("B", Some(1), "b"),
            make_device("C", Some(2), "c"),
        ];
        assert_eq!(find_duplicates(&devices), vec![1]);
    }

    #[test]
    fn parse_info_basic() {
        let (uid, devnum) = parse_info_response("uid=AABBCCDD11223344 devnum=42").unwrap();
        assert_eq!(uid, "AABBCCDD11223344");
        assert_eq!(devnum, Some(42));
    }

    #[test]
    fn parse_info_unassigned() {
        let (uid, devnum) = parse_info_response("uid=AABBCCDD11223344 devnum=255").unwrap();
        assert_eq!(uid, "AABBCCDD11223344");
        assert_eq!(devnum, None);
    }

    #[test]
    fn select_no_devices_fails() {
        let sel = DeviceSelector {
            uid: None,
            device: None,
            port: None,
        };
        assert!(select_device(&[], &sel).is_err());
    }
}
