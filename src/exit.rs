//! Process exit codes: 0 = ok, 1 = device error (`ERR` response),
//! 2 = connection error, 3 = argument / input error.

use pochi_sdk::SdkError;

pub const EXIT_DEVICE_ERROR: i32 = 1;
pub const EXIT_CONNECTION: i32 = 2;
pub const EXIT_ARGUMENT: i32 = 3;

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
        | SdkError::UnknownDevice(_) => EXIT_DEVICE_ERROR,
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
}
