use std::io;
use std::process::{Command, Output};
use std::sync::{LazyLock, Mutex};

use crate::error::AdbError;

/// All `adb -s` shells share this mutex; poller threads do not run shells in parallel.
static ADB_SHELL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Entry point for running `adb` commands.
pub struct Adb;

impl Adb {
    /// Verifies that `adb` is installed and runnable.
    pub fn check_available() -> Result<(), AdbError> {
        let output = run_adb(&["version"])?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(AdbError::VersionCheckFailed(stderr))
        }
    }

    /// Lists devices attached to the host via `adb devices -l`.
    pub fn list_devices() -> Result<Vec<crate::device::DeviceInfo>, AdbError> {
        let output = run_adb(&["devices", "-l"])?;
        let mut devices = crate::device::devices_from_output(&output)?;
        for device in devices.iter_mut().filter(|d| d.is_online()) {
            match check_shell(&device.serial) {
                Ok(()) => device.fetch_details(),
                Err(error) => {
                    device.state = crate::device::DeviceState::Other(format!(
                        "ADB check failed: {}",
                        error.user_message()
                    ))
                }
            }
        }
        Ok(devices)
    }

    /// Reboots a device, optionally targeting bootloader, recovery, or fastboot.
    pub fn reboot(serial: Option<&str>, target: Option<&str>) -> Result<String, AdbError> {
        let mut args = Vec::new();
        if let Some(s) = serial {
            args.push("-s");
            args.push(s);
        }
        args.push("reboot");
        if let Some(t) = target {
            if !t.is_empty() {
                args.push(t);
            }
        }
        let output = run_adb(&args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(AdbError::CommandFailed {
                command: format!("adb {}", args.join(" ")),
                stderr,
            })
        }
    }
}

/// A listed transport is not ready until it can actually execute a shell command.
fn check_shell(serial: &str) -> Result<(), AdbError> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = Command::new("adb")
        .args(["-s", serial, "shell", "echo", "dashboard-adb-ready"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_io_error)?;
    let started = Instant::now();
    loop {
        if child.try_wait().map_err(map_io_error)?.is_some() {
            let output = child.wait_with_output().map_err(map_io_error)?;
            return validate_shell_probe(&output);
        }
        if started.elapsed() >= Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AdbError::CommandFailed {
                command: "ADB connection check".into(),
                stderr: "Device did not respond within 5 seconds. Reconnect USB or check the wireless debugging address.".into(),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn validate_shell_probe(output: &Output) -> Result<(), AdbError> {
    if output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim() == "dashboard-adb-ready"
    {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(AdbError::CommandFailed {
            command: "ADB connection check".into(),
            stderr: if stderr.is_empty() {
                "Device did not return the expected ADB response".into()
            } else {
                stderr
            },
        })
    }
}

pub(crate) fn run_adb(args: &[&str]) -> Result<Output, AdbError> {
    let output = Command::new("adb")
        .args(args)
        .output()
        .map_err(map_io_error)?;

    if output.status.success() {
        Ok(output)
    } else {
        let command = format!("adb {}", args.join(" "));
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Err(AdbError::CommandFailed { command, stderr })
    }
}

pub(crate) fn run_adb_for_serial(serial: &str, args: &[&str]) -> Result<Output, AdbError> {
    let _guard = ADB_SHELL_LOCK.lock().expect("adb shell lock");
    let mut full_args = Vec::with_capacity(2 + args.len());
    full_args.push("-s");
    full_args.push(serial);
    full_args.extend_from_slice(args);
    run_adb(&full_args)
}

pub(crate) fn get_system_property(serial: &str, property: &str) -> Result<String, AdbError> {
    let result = run_adb_for_serial(serial, &["shell", "getprop", property])?;
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub(crate) fn get_optional_system_property(serial: &str, property: &str) -> Option<String> {
    match get_system_property(serial, property) {
        Ok(value) => {
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        Err(_) => None,
    }
}

fn map_io_error(err: io::Error) -> AdbError {
    if err.kind() == io::ErrorKind::NotFound {
        AdbError::NotFound
    } else {
        AdbError::Io(err)
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    #[test]
    fn requires_shell_response_not_just_success_exit() {
        let mut output = Output {
            status: std::process::ExitStatus::default(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert!(validate_shell_probe(&output).is_err());
        output.stdout = b"dashboard-adb-ready\r\n".to_vec();
        assert!(validate_shell_probe(&output).is_ok());
        output.stdout = b"unexpected response".to_vec();
        assert!(validate_shell_probe(&output).is_err());
    }
}
