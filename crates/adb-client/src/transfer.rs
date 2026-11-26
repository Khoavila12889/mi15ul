//! Folder browsing and cancellable ADB sync transfers, independent of metric polling.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};

#[derive(Debug, Clone)]
pub struct RemoteFolder {
    pub path: String,
    pub name: String,
}

pub fn quote_remote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn validate_remote(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.contains(['\0', '\r', '\n'])
        || path.split('/').any(|p| p == "..")
    {
        return Err("Use an absolute Android path without '..' or control characters".into());
    }
    Ok(())
}

pub fn list_remote_folders(serial: &str, path: &str) -> Result<Vec<RemoteFolder>, String> {
    validate_remote(path)?;
    let script = format!(
        "find {} -mindepth 1 -maxdepth 1 -type d -print0",
        quote_remote(&format!("{}/", path.trim_end_matches('/')))
    );
    let out = super::adb::run_adb_for_serial(serial, &["shell", &script])
        .map_err(|e| e.user_message())?;
    let mut folders: Vec<_> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| {
            let path = String::from_utf8_lossy(b).into_owned();
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            RemoteFolder { path, name }
        })
        .collect();
    folders.sort_by_key(|f| f.name.to_lowercase());
    Ok(folders)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Pull,
    Push,
}

#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub serial: String,
    pub direction: TransferDirection,
    pub remote: String,
    pub local: PathBuf,
    pub compression: bool,
}

pub enum TransferEvent {
    Output(String),
    Finished(Result<String, String>),
}

pub struct TransferJob {
    pub rx: Receiver<TransferEvent>,
    cancel: Arc<AtomicBool>,
}
impl TransferJob {
    pub fn start(request: TransferRequest) -> Self {
        let (tx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        thread::spawn(move || {
            let result = transfer(&request, &signal, |line| {
                let _ = tx.send(TransferEvent::Output(line));
            });
            let _ = tx.send(TransferEvent::Finished(result));
        });
        Self { rx, cancel }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for TransferJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn transfer(
    request: &TransferRequest,
    cancel: &AtomicBool,
    mut output: impl FnMut(String),
) -> Result<String, String> {
    validate_remote(&request.remote)?;
    if !request.local.is_absolute() {
        return Err("PC folder must be an absolute path".into());
    }
    if request.direction == TransferDirection::Push && !request.local.is_dir() {
        return Err("PC source folder does not exist".into());
    }
    if request.direction == TransferDirection::Pull && !request.local.is_dir() {
        return Err("PC destination folder does not exist".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    if request.direction == TransferDirection::Push {
        // Atomic reservation: never merge into or overwrite an existing phone folder.
        let script = format!("mkdir {}", quote_remote(&request.remote));
        super::adb::run_adb_for_serial(&request.serial, &["shell", &script])
            .map_err(|e| e.user_message())?;
    }
    let mut command = Command::new("adb");
    command.args([
        "-s",
        &request.serial,
        if request.direction == TransferDirection::Pull {
            "pull"
        } else {
            "push"
        },
    ]);
    if request.compression {
        command.args(["-z", "any"]);
    } else {
        command.arg("-Z");
    }
    if request.direction == TransferDirection::Pull {
        command.arg("-a");
        command.arg(&request.remote).arg(&request.local);
    } else {
        command.arg(&request.local).arg(&request.remote);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start ADB transfer: {e}"))?;
    let (tx, rx) = unbounded();
    for pipe in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut pipe = pipe;
            let mut buf = [0; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
    }
    drop(tx);
    let started = Instant::now();
    let mut tail = String::new();
    loop {
        for chunk in rx.try_iter() {
            tail.push_str(&chunk);
            if tail.len() > 8192 {
                tail = tail
                    .chars()
                    .rev()
                    .take(4096)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
            }
            output(chunk);
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled. Partial files are retained at the destination; this backup is incomplete.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // Readers drain after the child closes its handles, including its final summary.
                for chunk in rx.iter() {
                    tail.push_str(&chunk);
                    output(chunk);
                }
                if status.success() {
                    let summary = tail
                        .split(['\n', '\r'])
                        .rev()
                        .find(|s| !s.trim().is_empty())
                        .unwrap_or("");
                    return Ok(format!(
                        "Completed in {:.1}s. {}",
                        started.elapsed().as_secs_f64(),
                        summary.chars().take(800).collect::<String>()
                    ));
                }
                return Err(format!(
                    "ADB transfer failed: {}. Partial files may remain.",
                    tail.trim()
                ));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

pub fn local_folders(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| format!("Cannot read folder {}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            folders.push(entry.path());
        }
    }
    folders.sort();
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_paths_cannot_inject_shell_commands() {
        assert_eq!(
            quote_remote("/sdcard/John's $(touch x)"),
            "'/sdcard/John'\\''s $(touch x)'"
        );
        assert!(validate_remote("/sdcard/a b").is_ok());
        for p in ["relative", "/sdcard/../data", "/sdcard/a\ncmd", "/a\0b"] {
            assert!(validate_remote(p).is_err());
        }
    }

    #[test]
    #[ignore = "requires ANDROID_TRANSFER_TEST_SERIAL and an authorized test device"]
    fn live_folder_round_trip() {
        let serial = std::env::var("ANDROID_TRANSFER_TEST_SERIAL").expect("test serial");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target-gnu")
            .join(format!("transfer-test-{stamp}"));
        let source = root.join("folder with spaces");
        let destination = root.join("received");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let bytes: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
        std::fs::write(source.join("nested/payload.bin"), &bytes).unwrap();
        std::fs::write(source.join("hello.txt"), b"ADB folder round trip\n").unwrap();
        let remote = format!("/sdcard/Download/dashboard-test-{stamp}");
        let cancel = AtomicBool::new(false);
        let push = TransferRequest {
            serial: serial.clone(),
            direction: TransferDirection::Push,
            remote: remote.clone(),
            local: source,
            compression: true,
        };
        transfer(&push, &cancel, |s| eprint!("{s}")).unwrap();
        let folders = list_remote_folders(&serial, &remote).unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "folder with spaces");
        // A second push to an existing destination must fail, rather than overwrite.
        assert!(transfer(&push, &cancel, |_| {}).is_err());
        let pull = TransferRequest {
            serial,
            direction: TransferDirection::Pull,
            remote: folders[0].path.clone(),
            local: destination.clone(),
            compression: true,
        };
        transfer(&pull, &cancel, |s| eprint!("{s}")).unwrap();
        assert_eq!(
            std::fs::read(destination.join("folder with spaces/nested/payload.bin")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(destination.join("folder with spaces/hello.txt")).unwrap(),
            b"ADB folder round trip\n"
        );
        println!(
            "Verified fixture retained: {} and {}",
            root.display(),
            remote
        );
    }
}
