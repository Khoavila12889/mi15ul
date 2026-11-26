//! Package inventory and uninstall/restore actions.
//!
//! Uninstalling a package for the primary user (`--user 0`) works for both
//! user-installed apps and most preinstalled system apps without root. System
//! packages are kept in the system image and can be restored with
//! `cmd package install-existing`.

use crate::adb::run_adb_for_serial;
use crate::error::AdbError;

/// Removes `package` for the primary user only.
///
/// Prefer this over `pm uninstall <package>`: it also works for preinstalled
/// apps and leaves the APK in the system image so it can be restored.
pub fn uninstall_package(serial: &str, package: &str) -> Result<(), AdbError> {
    validate_package_name(package)?;

    let output = run_adb_for_serial(
        serial,
        &["shell", "pm", "uninstall", "--user", "0", package],
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_action_output("uninstall", &stdout)
}

/// Reinstalls a package for the primary user if it still exists on the device.
pub fn restore_package(serial: &str, package: &str) -> Result<(), AdbError> {
    validate_package_name(package)?;

    let output = run_adb_for_serial(
        serial,
        &["shell", "cmd", "package", "install-existing", package],
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_action_output("restore", &stdout)
}

/// Rejects names that could be interpreted as extra shell arguments.
fn validate_package_name(package: &str) -> Result<(), AdbError> {
    let valid = !package.is_empty()
        && package.len() <= 255
        && package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

    if valid {
        Ok(())
    } else {
        Err(AdbError::PackageActionFailed(format!(
            "Invalid package name: {package}"
        )))
    }
}

/// `pm uninstall` exits 0 even for some failures, so inspect stdout as well.
fn parse_action_output(action: &str, stdout: &str) -> Result<(), AdbError> {
    let trimmed = stdout.trim();

    if trimmed.split_whitespace().any(|word| word == "Success")
        || trimmed.contains("installed for user")
    {
        return Ok(());
    }

    let detail = if trimmed.is_empty() {
        "no output".to_string()
    } else {
        trimmed.to_string()
    };
    Err(AdbError::PackageActionFailed(format!(
        "{action} failed: {detail}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_uninstall_success() {
        assert!(parse_action_output("uninstall", "Success\n").is_ok());
    }

    #[test]
    fn accepts_restore_success() {
        assert!(
            parse_action_output("restore", "Package com.example installed for user: 0\n").is_ok()
        );
    }

    #[test]
    fn reports_failure_text() {
        let err = parse_action_output("uninstall", "Failure [DELETE_FAILED_INTERNAL_ERROR]")
            .expect_err("failure output must not parse as success");
        assert_eq!(
            err.user_message(),
            "uninstall failed: Failure [DELETE_FAILED_INTERNAL_ERROR]"
        );
    }

    #[test]
    fn rejects_packages_with_shell_metacharacters() {
        assert!(validate_package_name("com.example.app").is_ok());
        assert!(validate_package_name("com.example;reboot").is_err());
        assert!(validate_package_name("").is_err());
    }
}
