//! Package inventory and uninstall/restore actions.
//!
//! Uninstalling a package for the primary user (`--user 0`) works for both
//! user-installed apps and most preinstalled system apps without root. System
//! packages are kept in the system image and can be restored with
//! `cmd package install-existing`.

use crate::adb::run_adb_for_serial;
use crate::error::AdbError;
use crate::hyperos::{
    is_core_protected_package, parse_default_ime_package, parse_default_launcher_package,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    Uninstalled,
    Disabled,
    Suspended,
}

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

/// Queries active IME and default HOME launcher packages that should remain protected.
pub fn query_active_protected_packages(serial: &str) -> Vec<String> {
    let mut protected = Vec::new();
    if let Ok(out) = run_adb_for_serial(
        serial,
        &["shell", "settings", "get", "secure", "default_input_method"],
    ) {
        if let Some(ime) = parse_default_ime_package(&String::from_utf8_lossy(&out.stdout)) {
            protected.push(ime);
        }
    }
    if let Ok(out) = run_adb_for_serial(
        serial,
        &[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-c",
            "android.intent.category.HOME",
            "-a",
            "android.intent.action.MAIN",
        ],
    ) {
        if let Some(launcher) =
            parse_default_launcher_package(&String::from_utf8_lossy(&out.stdout))
        {
            if !protected.contains(&launcher) {
                protected.push(launcher);
            }
        }
    }
    protected
}

/// Deep uninstall with HyperOS /data/app and protected system fallbacks:
/// 1) `pm uninstall --user 0`
/// 2) `pm uninstall -k --user 0`
/// 3) `pm uninstall-system-updates` + `pm uninstall --user 0`
/// 4) `pm uninstall <pkg>` (for /data/app preloads)
/// 5) `pm disable-user --user 0` + `am force-stop`
/// 6) `pm suspend --user 0` + `am force-stop`
pub fn uninstall_package_deep(serial: &str, package: &str) -> Result<UninstallOutcome, AdbError> {
    validate_package_name(package)?;
    if is_core_protected_package(package) {
        return Err(AdbError::PackageActionFailed(format!(
            "Protected core package: {package}"
        )));
    }

    let try_uninstall = |args: &[&str]| -> Result<bool, String> {
        let out = run_adb_for_serial(serial, args).map_err(|e| e.user_message())?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if parse_action_output("uninstall", &text).is_ok() {
            Ok(true)
        } else {
            Err(text.trim().to_string())
        }
    };

    let mut last_err = String::new();
    for args in [
        &["shell", "pm", "uninstall", "--user", "0", package][..],
        &["shell", "pm", "uninstall", "-k", "--user", "0", package][..],
    ] {
        match try_uninstall(args) {
            Ok(true) => return Ok(UninstallOutcome::Uninstalled),
            Ok(false) => {}
            Err(e) => last_err = e,
        }
    }

    let _ = run_adb_for_serial(
        serial,
        &["shell", "pm", "uninstall-system-updates", package],
    );
    for args in [
        &["shell", "pm", "uninstall", "--user", "0", package][..],
        &["shell", "pm", "uninstall", package][..],
    ] {
        match try_uninstall(args) {
            Ok(true) => return Ok(UninstallOutcome::Uninstalled),
            Ok(false) => {}
            Err(e) => last_err = e,
        }
    }

    // Fallback: disable-user + force-stop
    if let Ok(out) = run_adb_for_serial(
        serial,
        &["shell", "pm", "disable-user", "--user", "0", package],
    ) {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if parse_disable_output(&text).is_ok() {
            let _ = run_adb_for_serial(serial, &["shell", "am", "force-stop", package]);
            return Ok(UninstallOutcome::Disabled);
        }
        last_err = text.trim().to_string();
    }

    // Fallback: suspend + force-stop
    if let Ok(out) = run_adb_for_serial(
        serial,
        &["shell", "pm", "suspend", "--user", "0", package],
    ) {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if parse_suspend_output(&text).is_ok() {
            let _ = run_adb_for_serial(serial, &["shell", "am", "force-stop", package]);
            return Ok(UninstallOutcome::Suspended);
        }
        last_err = text.trim().to_string();
    }

    Err(AdbError::PackageActionFailed(format!(
        "uninstall/disable/suspend failed for {package}: {last_err}"
    )))
}

/// Four-layer restore: `install-existing` + `unhide` + `unsuspend` + `enable`.
pub fn restore_package_deep(serial: &str, package: &str) -> Result<(), AdbError> {
    validate_package_name(package)?;

    let mut restored = false;
    let mut last_err = String::new();

    for args in [
        &["shell", "cmd", "package", "install-existing", "--user", "0", package][..],
        &["shell", "cmd", "package", "install-existing", package][..],
    ] {
        if let Ok(out) = run_adb_for_serial(serial, args) {
            let text = String::from_utf8_lossy(&out.stdout);
            if parse_action_output("restore", &text).is_ok() {
                restored = true;
                break;
            }
            last_err = text.trim().to_string();
        }
    }

    if let Ok(out) = run_adb_for_serial(serial, &["shell", "pm", "unhide", "--user", "0", package])
    {
        let low = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        if low.contains("false") {
            restored = true;
        }
    }

    if let Ok(out) =
        run_adb_for_serial(serial, &["shell", "pm", "unsuspend", "--user", "0", package])
    {
        let low = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        if low.contains("false") || low.contains("unsuspended") || low.contains("new suspended state") {
            restored = true;
        }
    }

    if let Ok(out) = run_adb_for_serial(serial, &["shell", "pm", "enable", "--user", "0", package])
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let low = text.to_ascii_lowercase();
        if (low.contains("enabled") || low.contains("new state"))
            && !low.contains("exception")
            && !low.contains("error")
        {
            restored = true;
        } else if !restored {
            last_err = text.trim().to_string();
        }
    }

    if restored {
        Ok(())
    } else {
        Err(AdbError::PackageActionFailed(format!(
            "restore failed for {package}: {last_err}"
        )))
    }
}

fn parse_disable_output(stdout: &str) -> Result<(), AdbError> {
    let trimmed = stdout.trim();
    let low = trimmed.to_ascii_lowercase();
    if (low.contains("disabled") || low.contains("new state"))
        && !low.contains("exception")
        && !low.contains("error")
        && !low.contains("failure")
    {
        Ok(())
    } else {
        Err(AdbError::PackageActionFailed(format!(
            "disable failed: {trimmed}"
        )))
    }
}

fn parse_suspend_output(stdout: &str) -> Result<(), AdbError> {
    let trimmed = stdout.trim();
    let low = trimmed.to_ascii_lowercase();
    if (low.contains("suspended") || low.contains("new suspended state") || low.contains("true"))
        && !low.contains("exception")
        && !low.contains("error")
        && !low.contains("failure")
    {
        Ok(())
    } else {
        Err(AdbError::PackageActionFailed(format!(
            "suspend failed: {trimmed}"
        )))
    }
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

    #[test]
    fn parses_disable_and_suspend_outputs() {
        assert!(
            parse_disable_output("Package com.xiaomi.gamecenter new state: disabled-user\n")
                .is_ok()
        );
        assert!(parse_disable_output("SecurityException: Permission denied").is_err());
        assert!(
            parse_suspend_output("Package com.xiaomi.finddevice new suspended state: true\n")
                .is_ok()
        );
        assert!(parse_suspend_output("Failure [not installed]").is_err());
    }
}
