//! Device enumeration and selection (`--port`, `--uid`, `--device`). The
//! scanning, probing and connecting is done by `tcapi-sdk`; this module adds
//! the CLI's selection rules and error messages.

use anyhow::{Context, Result};
use tcapi_sdk::{discovery, Connection, DeviceInfo, SdkError};

#[derive(Debug, Clone)]
pub struct DeviceSelector {
    pub uid: Option<String>,
    pub device: Option<u8>,
    pub port: Option<String>,
}

/// Enumerate and probe all Tcapi-DIO devices. Ports that fail to probe (for
/// example held open by another process) are reported as warnings; they do
/// not abort the scan.
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>> {
    let found = discovery::discover_detailed()?;
    for failure in &found.failures {
        eprintln!(
            "warning: failed to probe {}: {}",
            failure.port, failure.error
        );
    }
    Ok(found.devices)
}

/// Pick one device from an enumerated list: `--port`, then `--uid`, then
/// `--device`, otherwise the sole device.
pub fn select_device(devices: &[DeviceInfo], selector: &DeviceSelector) -> Result<DeviceInfo> {
    if let Some(port) = &selector.port {
        return devices
            .iter()
            .find(|d| d.port == *port)
            .cloned()
            .ok_or(SdkError::NoDevice)
            .with_context(|| format!("no Tcapi-DIO device on port {port}"));
    }
    if let Some(uid) = &selector.uid {
        return discovery::find_by_uid(devices, uid)
            .with_context(|| format!("no Tcapi-DIO device with UID {uid}"));
    }
    if let Some(num) = selector.device {
        return discovery::find_by_device_number(devices, num).with_context(|| {
            format!(
                "cannot select device number {num}; if several devices share it, \
                 use --uid or --port to disambiguate"
            )
        });
    }
    Ok(discovery::select_sole_device(devices)?)
}

/// Connect to the selected device. `--port` opens the path directly (no
/// enumeration); the returned info then only knows the port.
pub fn open_device(selector: &DeviceSelector) -> Result<(DeviceInfo, Connection)> {
    if let Some(port) = &selector.port {
        let conn = open_port(port)?;
        let info = DeviceInfo {
            uid: String::new(),
            device_number: None,
            port: port.clone(),
            version: None,
        };
        return Ok((info, conn));
    }
    let devices = enumerate_devices()?;
    let selected = select_device(&devices, selector)?;
    let conn = open_port(&selected.port)?;
    Ok((selected, conn))
}

/// Open one serial port and verify the device speaks `proto=2`.
pub fn open_port(port: &str) -> Result<Connection> {
    Connection::open(port).with_context(|| format!("failed to open {port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(uid: &str, devnum: Option<u8>, port: &str) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_string(),
            device_number: devnum,
            port: port.to_string(),
            version: Some("0.1.3".to_string()),
        }
    }

    fn selector(uid: Option<&str>, device: Option<u8>, port: Option<&str>) -> DeviceSelector {
        DeviceSelector {
            uid: uid.map(str::to_string),
            device,
            port: port.map(str::to_string),
        }
    }

    fn pair() -> Vec<DeviceInfo> {
        vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(2), "/dev/ttyACM1"),
        ]
    }

    #[test]
    fn select_single_device() {
        let devices = vec![make_device("AABB", Some(1), "/dev/ttyACM0")];
        let result = select_device(&devices, &selector(None, None, None)).unwrap();
        assert_eq!(result.uid, "AABB");
    }

    #[test]
    fn select_by_uid_is_case_insensitive() {
        let result = select_device(&pair(), &selector(Some("ccdd"), None, None)).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_by_device_number() {
        let result = select_device(&pair(), &selector(None, Some(2), None)).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_by_port() {
        let result = select_device(&pair(), &selector(None, None, Some("/dev/ttyACM1"))).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_port_takes_priority_over_uid() {
        let result =
            select_device(&pair(), &selector(Some("AABB"), None, Some("/dev/ttyACM1"))).unwrap();
        assert_eq!(result.uid, "CCDD");
    }

    #[test]
    fn select_multiple_without_selector_fails() {
        let err = select_device(&pair(), &selector(None, None, None)).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SdkError>(),
            Some(SdkError::MultipleDevices)
        ));
    }

    #[test]
    fn select_duplicate_device_number_fails_and_hints_at_uid() {
        let devices = vec![
            make_device("AABB", Some(1), "/dev/ttyACM0"),
            make_device("CCDD", Some(1), "/dev/ttyACM1"),
        ];
        let err = select_device(&devices, &selector(None, Some(1), None)).unwrap_err();
        assert!(format!("{err:#}").contains("--uid"));
        assert!(matches!(
            err.downcast_ref::<SdkError>(),
            Some(SdkError::MultipleDevices)
        ));
    }

    #[test]
    fn select_unknown_targets_report_no_device() {
        for sel in [
            selector(Some("EEFF"), None, None),
            selector(None, Some(9), None),
            selector(None, None, Some("/dev/ttyACM9")),
        ] {
            let err = select_device(&pair(), &sel).unwrap_err();
            assert!(matches!(
                err.downcast_ref::<SdkError>(),
                Some(SdkError::NoDevice)
            ));
        }
    }

    #[test]
    fn select_from_empty_list_fails() {
        let err = select_device(&[], &selector(None, None, None)).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SdkError>(),
            Some(SdkError::NoDevice)
        ));
    }
}
