//! Process exit codes: 0 = ok, 1 = device error (`ERR` response),
//! 2 = connection error, 3 = argument / input error, 4 = the failsafe latch
//! refused the command (`ERR FAILSAFE`), 5 = an exclusive output group would
//! be violated (`ERR EXCLUSIVE`).

use pochi_sdk::SdkError;
use serde_json::{json, Value};

pub const EXIT_DEVICE_ERROR: i32 = 1;
pub const EXIT_CONNECTION: i32 = 2;
pub const EXIT_ARGUMENT: i32 = 3;
pub const EXIT_FAILSAFE: i32 = 4;
pub const EXIT_EXCLUSIVE: i32 = 5;

/// Classify an error by the first [`SdkError`] in its cause chain. Errors
/// that carry no SDK error (bad arguments, unreadable or invalid files) are
/// input errors.
pub fn exit_code(err: &anyhow::Error) -> i32 {
    let Some(sdk) = err.chain().find_map(|e| e.downcast_ref::<SdkError>()) else {
        return EXIT_ARGUMENT;
    };
    match sdk {
        SdkError::BadCommand
        | SdkError::BadParam
        | SdkError::BadMap
        | SdkError::Busy
        | SdkError::Flash
        | SdkError::TooLong
        | SdkError::NotArmed
        | SdkError::SyncState
        | SdkError::SyncTimeout
        | SdkError::NoResource
        | SdkError::NotRunning
        | SdkError::TransportNotSupported
        | SdkError::SafetyMismatch(_)
        | SdkError::UnknownDevice(_) => EXIT_DEVICE_ERROR,
        SdkError::Failsafe => EXIT_FAILSAFE,
        SdkError::Exclusive { .. } => EXIT_EXCLUSIVE,
        SdkError::Connection(_)
        | SdkError::Io(_)
        | SdkError::Timeout
        | SdkError::NoDevice
        | SdkError::MultipleDevices
        | SdkError::Disconnected
        | SdkError::UnsupportedFirmware(_) => EXIT_CONNECTION,
        SdkError::Parse(_) | SdkError::InvalidArgument(_) => EXIT_ARGUMENT,
    }
}

/// The `--json` form of a failed command: `{"error": "<message>"}`, plus
/// `"kind"` for the two refusals that have an exit code of their own
/// (`"failsafe"`, `"exclusive"`), and `"group"` / `"mask"` (numbers) for an
/// exclusive-group violation.
pub fn error_json(err: &anyhow::Error) -> Value {
    let mut value = json!({"error": format!("{err:#}")});
    let sdk = err.chain().find_map(|e| e.downcast_ref::<SdkError>());
    match sdk {
        Some(SdkError::Failsafe) => value["kind"] = json!("failsafe"),
        Some(SdkError::Exclusive { group, mask }) => {
            value["kind"] = json!("exclusive");
            value["group"] = json!(group);
            value["mask"] = json!(mask);
        }
        _ => {}
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    fn code(e: SdkError) -> i32 {
        exit_code(&anyhow::Error::new(e))
    }

    #[test]
    fn device_errors_exit_1() {
        assert_eq!(code(SdkError::BadParam), EXIT_DEVICE_ERROR);
        assert_eq!(code(SdkError::NotRunning), EXIT_DEVICE_ERROR);
        assert_eq!(
            code(SdkError::UnknownDevice("FOO".into())),
            EXIT_DEVICE_ERROR
        );
    }

    #[test]
    fn failsafe_and_exclusive_have_their_own_codes() {
        assert_eq!(EXIT_FAILSAFE, 4);
        assert_eq!(EXIT_EXCLUSIVE, 5);
        assert_eq!(code(SdkError::Failsafe), EXIT_FAILSAFE);
        assert_eq!(
            code(SdkError::Exclusive {
                group: 1,
                mask: 0xc
            }),
            EXIT_EXCLUSIVE
        );
    }

    #[test]
    fn other_safety_errors_exit_1() {
        assert_eq!(code(SdkError::TransportNotSupported), EXIT_DEVICE_ERROR);
        assert_eq!(
            code(SdkError::SafetyMismatch("wdog_ms".into())),
            EXIT_DEVICE_ERROR
        );
    }

    #[test]
    fn connection_errors_exit_2() {
        assert_eq!(code(SdkError::NoDevice), EXIT_CONNECTION);
        assert_eq!(code(SdkError::MultipleDevices), EXIT_CONNECTION);
        assert_eq!(code(SdkError::Timeout), EXIT_CONNECTION);
        assert_eq!(code(SdkError::Disconnected), EXIT_CONNECTION);
        assert_eq!(
            code(SdkError::UnsupportedFirmware("proto=1".into())),
            EXIT_CONNECTION
        );
    }

    #[test]
    fn input_errors_exit_3() {
        assert_eq!(code(SdkError::InvalidArgument("x".into())), EXIT_ARGUMENT);
        assert_eq!(code(SdkError::Parse("x".into())), EXIT_ARGUMENT);
        assert_eq!(exit_code(&anyhow::anyhow!("plain message")), EXIT_ARGUMENT);
    }

    #[test]
    fn context_does_not_hide_the_sdk_error() {
        let err = Err::<(), _>(SdkError::NoDevice)
            .context("no Pochi-DIO device with UID X")
            .unwrap_err();
        assert_eq!(exit_code(&err), EXIT_CONNECTION);
    }

    #[test]
    fn error_json_has_only_the_message_for_ordinary_errors() {
        let err = anyhow::Error::new(SdkError::BadParam);
        let value = error_json(&err);
        assert_eq!(value.as_object().unwrap().len(), 1);
        assert!(value["error"].as_str().unwrap().contains("BAD_PARAM"));
    }

    #[test]
    fn error_json_marks_a_failsafe_refusal() {
        let err = Err::<(), _>(SdkError::Failsafe)
            .context("write refused")
            .unwrap_err();
        let value = error_json(&err);
        assert_eq!(value["kind"], "failsafe");
        assert!(value["error"]
            .as_str()
            .unwrap()
            .starts_with("write refused"));
        assert!(value.get("group").is_none());
    }

    #[test]
    fn error_json_carries_group_and_mask_of_an_exclusive_violation() {
        let err = anyhow::Error::new(SdkError::Exclusive {
            group: 1,
            mask: 0xc,
        });
        let value = error_json(&err);
        assert_eq!(value["kind"], "exclusive");
        assert_eq!(value["group"], 1);
        assert_eq!(value["mask"], 12);
    }
}
