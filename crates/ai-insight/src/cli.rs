//! Runs the locally installed Codex/OpenCode CLIs with their existing login.
//!
//! Both tools are npm shims on Windows, so they are launched through `cmd /C`
//! to let `PATHEXT` resolve `codex.cmd` / `opencode.cmd`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::client::InsightError;
use crate::config::InsightConfig;

const CLI_TIMEOUT: Duration = Duration::from_secs(150);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_ERROR_CHARS: usize = 400;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Runs `codex exec` read-only and returns the last assistant message.
pub(crate) fn run_codex(config: &InsightConfig, prompt: &str) -> Result<String, InsightError> {
    let out_path = temp_path("codex-last");
    let stdout_path = temp_path("codex-stdout");
    let stderr_path = temp_path("codex-stderr");
    let _guard = TempFiles(vec![
        out_path.clone(),
        stdout_path.clone(),
        stderr_path.clone(),
    ]);

    let mut command = cli_command("codex");
    command
        .arg("exec")
        .arg("--skip-git-repo-check")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--ephemeral")
        .arg("--color")
        .arg("never")
        .arg("-C")
        .arg(std::env::temp_dir())
        .arg("-o")
        .arg(&out_path)
        .arg("-");
    let model = config.model.trim();
    if !model.is_empty() {
        command.arg("-m").arg(model);
    }
    attach_stdio(&mut command, &stdout_path, &stderr_path);

    run_command(command, prompt, "Codex CLI", &stdout_path, &stderr_path)?;

    let last_message = read_clean(&out_path);
    if !last_message.is_empty() {
        return Ok(last_message);
    }
    Ok(read_clean(&stdout_path))
}

/// Runs `opencode run` and returns the assistant text printed on stdout.
pub(crate) fn run_opencode(config: &InsightConfig, prompt: &str) -> Result<String, InsightError> {
    let stdout_path = temp_path("opencode-stdout");
    let stderr_path = temp_path("opencode-stderr");
    let _guard = TempFiles(vec![stdout_path.clone(), stderr_path.clone()]);

    let mut command = cli_command("opencode");
    command
        .arg("run")
        .arg("--standalone")
        .arg("--format")
        .arg("default");
    let model = config.model.trim();
    if !model.is_empty() {
        command.arg("-m").arg(model);
    }
    attach_stdio(&mut command, &stdout_path, &stderr_path);

    run_command(command, prompt, "OpenCode CLI", &stdout_path, &stderr_path)?;
    Ok(read_clean(&stdout_path))
}

fn cli_command(program: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

fn attach_stdio(command: &mut Command, stdout_path: &Path, stderr_path: &Path) {
    command.stdin(Stdio::piped());
    if let Ok(file) = fs::File::create(stdout_path) {
        command.stdout(Stdio::from(file));
    } else {
        command.stdout(Stdio::null());
    }
    if let Ok(file) = fs::File::create(stderr_path) {
        command.stderr(Stdio::from(file));
    } else {
        command.stderr(Stdio::null());
    }
}

fn run_command(
    mut command: Command,
    prompt: &str,
    label: &str,
    _stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(), InsightError> {
    let mut child = command.spawn().map_err(|err| {
        InsightError::Cli(format!("{label} is not installed or not on PATH ({err})"))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        // A very fast failure closes stdin early; the exit status still tells
        // the real story, so a broken pipe is not fatal here.
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let status = wait_with_timeout(&mut child, label)?;
    if status.success() {
        return Ok(());
    }

    let detail = read_clean(stderr_path);
    let detail = if detail.is_empty() {
        "no error output".to_string()
    } else {
        truncate_chars(&detail, MAX_ERROR_CHARS)
    };
    Err(InsightError::Cli(format!(
        "{label} exited with {}: {detail}",
        describe_status(status)
    )))
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    label: &str,
) -> Result<ExitStatus, InsightError> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(err) => {
                return Err(InsightError::Cli(format!(
                    "failed to wait for {label}: {err}"
                )));
            }
        }
        if started.elapsed() >= CLI_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(InsightError::Cli(format!(
                "{label} timed out after {}s",
                CLI_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn describe_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("status {code}"),
        None => "a signal".to_string(),
    }
}

fn read_clean(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|text| strip_ansi(&text).trim().to_string())
        .unwrap_or_default()
}

fn temp_path(tag: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "android-dashboard-insight-{tag}-{}-{seq}.log",
        std::process::id()
    ))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push('…');
    truncated
}

/// Removes ANSI CSI sequences (`ESC [ ... final`) from CLI output.
fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if ch != '\r' {
                output.push(ch);
            }
            continue;
        }
        if let Some('[') = chars.next() {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    output
}

/// Deletes temporary output files when dropped, even on the error path.
struct TempFiles(Vec<PathBuf>);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_color_sequences() {
        assert_eq!(
            strip_ansi("\u{1b}[91m\u{1b}[1mError: boom\u{1b}[0m"),
            "Error: boom"
        );
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel…");
        assert_eq!(truncate_chars("xin chào", 3), "xin…");
    }
}
