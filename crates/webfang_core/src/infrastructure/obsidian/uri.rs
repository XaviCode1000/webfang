//! Obsidian URI protocol support.
//!
//! Opens notes directly in Obsidian using the `obsidian://` URI scheme.
//!
//! URI format: `obsidian://open?vault=<vault_name>&file=<file_path>`

use std::path::Path;

/// Comprehensive percent-encoding for Obsidian URI parameters.
///
/// Whitelist-based: only RFC 3986 unreserved characters (`A-Z a-z 0-9 - _ . ~`)
/// plus `/` (Obsidian needs slashes unencoded in file paths) survive verbatim.
/// Every other ASCII character — including all cmd.exe metacharacters
/// (`| > < ^ ; ( ) & = # ? % + space`) — is percent-encoded so it can never be
/// interpreted by a shell. This neutralizes shell metacharacters (Windows
/// cmd.exe safety) while preserving `/` for Obsidian file paths. Non-ASCII
/// characters are UTF-8 percent-encoded byte by byte.
fn encode_obsidian_param(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            // RFC 3986 unreserved chars + '/' (Obsidian needs slashes unencoded in file paths).
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => out.push(ch),
            // Every other ASCII char — including cmd.exe metacharacters | > < ^ ; ( ) & = # ? % +
            // and space — is percent-encoded so it can never be interpreted by a shell.
            c if c.is_ascii() => out.push_str(&format!("%{:02X}", c as u32)),
            // Non-ASCII: UTF-8 percent-encode each byte.
            c => {
                let mut buf = [0u8; 4];
                for &byte in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            },
        }
    }
    out
}

/// Build an Obsidian URI from vault name and file path.
///
/// # Arguments
/// - `vault_name` — Name of the Obsidian vault (folder name, not full path)
/// - `file_path` — Path to the note relative to the vault root (without extension)
///
/// # Returns
/// URI string ready for opening
pub fn build_obsidian_uri(vault_name: &str, file_path: &str) -> String {
    format!(
        "obsidian://open?vault={}&file={}",
        encode_obsidian_param(vault_name),
        encode_obsidian_param(file_path)
    )
}

/// Validate Obsidian URI inputs, rejecting ASCII control characters.
///
/// Shell metacharacters are neutralized by `encode_obsidian_param` (percent-encoded),
/// so they are safe in the URI. Control characters (newline, null byte, etc.) have no
/// legitimate place in a vault name or note path and are rejected outright as a signal
/// of malformed or hostile input.
///
/// # Errors
/// Returns `Err` with a user-facing (Spanish) message if either input contains an
/// ASCII control character.
pub fn validate_obsidian_input(vault_name: &str, file_path: &str) -> Result<(), String> {
    for (label, value) in [("vault_name", vault_name), ("file_path", file_path)] {
        if value.chars().any(|c| c.is_ascii_control()) {
            return Err(format!(
                "{label} contiene caracteres de control no permitidos"
            ));
        }
    }
    Ok(())
}

/// Result of dispatching an Obsidian URI to the OS handler (issue #591).
///
/// Replaces the bare `()` success value so callers can distinguish between
/// "command dispatched" and "command failed to start".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStatus {
    /// The OS handler command was spawned and exited successfully.
    /// The URI was delivered to the system's protocol handler.
    Dispatched,
    /// The handler command was spawned but exited with a non-zero status.
    /// Obsidian may not be installed or the URI scheme is unregistered.
    HandlerFailed,
}

/// Open a note in Obsidian using the URI protocol.
///
/// Uses `xdg-open` on Linux, `open` on macOS, `explorer.exe` on Windows
/// (XP-S-01/XP-C-04, #1608 — see [`dispatch_windows`]).
/// Spawns the handler, waits (bounded — XP-S-06) for exit, and reports
/// whether the system's protocol handler accepted the URI (issue #591 —
/// honest dispatch).
///
/// # Arguments
/// - `uri` — The obsidian:// URI to open
///
/// # Returns
/// `Ok(DispatchStatus::Dispatched)` if the handler exited cleanly,
/// `Ok(DispatchStatus::HandlerFailed)` if the handler exited non-zero
/// (Obsidian likely not installed), `Err(String)` if the command failed to start.
pub fn open_in_obsidian(uri: &str) -> Result<DispatchStatus, String> {
    // XP-C-04 (#1608): platform dispatch is `#[cfg]`-separated per function,
    // not a runtime `cfg!()` branch. The `cfg!` version compiles every
    // branch on every target, so Windows-only code had to type-check on
    // Linux while never running there; `#[cfg]` gates foreign-OS code out of
    // the build entirely.
    #[cfg(target_os = "windows")]
    return dispatch_windows(uri);
    #[cfg(target_os = "macos")]
    return dispatch_macos(uri);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return dispatch_xdg_open(uri);
}

/// Upper bound on how long the protocol-handler dispatch may run (XP-S-06).
///
/// `xdg-open`/`open` exit quickly after handing the URI to the OS handler;
/// a wedged handler must not hang the CLI indefinitely. On expiry the child
/// is killed and the dispatch is reported as [`DispatchStatus::Dispatched`]
/// — the URI was already handed over, so treating the hang as a failure
/// would misreport. A `warn!` is emitted for diagnostics.
const HANDLER_DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Poll interval while waiting for the handler to exit.
const HANDLER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Bounded, synchronous wait for a spawned handler (XP-S-06, #1608).
///
/// Returns `Some(Ok(status))` on exit, `Some(Err(e))` on a wait error, and
/// `None` if `timeout` elapsed first. On `None` the child is killed (and
/// reaped) before returning — no orphaned handler is left behind.
fn wait_for_exit_with_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> Option<std::io::Result<std::process::ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(Ok(status)),
            Ok(None) => {},
            Err(e) => return Some(Err(e)),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(HANDLER_POLL_INTERVAL);
    }
}

/// Spawn `program args` with silenced stdio and classify its exit (issue
/// #591 honest dispatch), bounded by [`HANDLER_DISPATCH_TIMEOUT`].
#[cfg(not(target_os = "windows"))]
fn dispatch_via(program: &str, args: &[&str]) -> Result<DispatchStatus, String> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to launch Obsidian handler: {e}"))?;

    match wait_for_exit_with_timeout(&mut child, HANDLER_DISPATCH_TIMEOUT) {
        Some(Ok(status)) if status.success() => Ok(DispatchStatus::Dispatched),
        Some(Ok(status)) => {
            tracing::debug!(status = %status, program, "obsidian protocol handler exited non-zero");
            Ok(DispatchStatus::HandlerFailed)
        },
        Some(Err(e)) => Err(format!("obsidian handler wait failed: {e}")),
        None => {
            tracing::warn!(
                program,
                timeout_secs = HANDLER_DISPATCH_TIMEOUT.as_secs(),
                "obsidian protocol handler did not exit in time — killed; treating as dispatched"
            );
            Ok(DispatchStatus::Dispatched)
        },
    }
}

/// Linux: `xdg-open` takes the URI as one argv entry (no shell involved).
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn dispatch_xdg_open(uri: &str) -> Result<DispatchStatus, String> {
    dispatch_via("xdg-open", &[uri])
}

/// macOS: `open` takes the URI as one argv entry (no shell involved).
#[cfg(target_os = "macos")]
fn dispatch_macos(uri: &str) -> Result<DispatchStatus, String> {
    dispatch_via("open", &[uri])
}

/// Windows (XP-S-01, #1608): `explorer.exe <uri>`.
///
/// The previous `cmd /C start "" <uri>` was broken for URIs with a
/// structural `&`: the URI reaches cmd.exe UNQUOTED (std's arg escaping
/// only quotes args containing spaces/quotes), and cmd.exe parses the raw
/// `&` as a command separator — `start "" obsidian://open?vault=X` ran
/// truncated while `file=Y` was attempted as a separate command.
///
/// `explorer.exe` receives the URI as ONE argv entry and forwards it to the
/// default `obsidian://` protocol handler; no shell re-parses it, so the
/// structural `&` survives intact. (The empty `""` title dance exists only
/// for `start`, and is not needed here.)
///
/// Known limitation, documented rather than hidden: `explorer.exe` exit
/// codes are unreliable for protocol dispatch (it commonly returns 1 on
/// success), so Windows cannot distinguish [`DispatchStatus::HandlerFailed`]
/// from [`DispatchStatus::Dispatched`] by exit status — a successful spawn
/// is reported as dispatched and the exit status is logged. NEEDS RUNTIME
/// VERIFICATION on the Windows advisory CI lane (#1608).
#[cfg(target_os = "windows")]
fn dispatch_windows(uri: &str) -> Result<DispatchStatus, String> {
    let mut command = std::process::Command::new("explorer.exe");
    command
        .arg(uri)
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to launch Obsidian handler: {e}"))?;

    match wait_for_exit_with_timeout(&mut child, HANDLER_DISPATCH_TIMEOUT) {
        Some(Ok(status)) => {
            tracing::debug!(
                status = %status,
                "explorer.exe protocol dispatch exit status (unreliable by design)"
            );
            Ok(DispatchStatus::Dispatched)
        },
        Some(Err(e)) => Err(format!("obsidian handler wait failed: {e}")),
        None => {
            tracing::warn!(
                timeout_secs = HANDLER_DISPATCH_TIMEOUT.as_secs(),
                "explorer.exe did not exit in time — killed; treating as dispatched"
            );
            Ok(DispatchStatus::Dispatched)
        },
    }
}

/// Extract vault name from a vault path (last directory component).
///
/// # Example
/// `/home/user/Obsidian/MyVault` → `MyVault`
#[must_use]
pub fn extract_vault_name(vault_path: &Path) -> String {
    vault_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Open a note in Obsidian from vault path and relative file path.
///
/// Convenience function that combines `extract_vault_name`, `build_obsidian_uri`,
/// and `open_in_obsidian`.
///
/// # Arguments
/// - `vault_path` — Full path to the Obsidian vault
/// - `file_path` — Path to the note relative to the vault root
///
/// # Returns
/// `Ok(DispatchStatus)` with the dispatch outcome, `Err(String)` on spawn failure
pub fn open_note(vault_path: &Path, file_path: &Path) -> Result<DispatchStatus, String> {
    let vault_name = extract_vault_name(vault_path);

    // Get relative path from vault root
    let relative = if file_path.is_absolute() {
        file_path.strip_prefix(vault_path).unwrap_or(file_path)
    } else {
        file_path
    };

    // Convert to string, normalize separators, remove .md extension
    let file_str = relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches(".md")
        .to_string();

    validate_obsidian_input(&vault_name, &file_str)?;

    let uri = build_obsidian_uri(&vault_name, &file_str);
    open_in_obsidian(&uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_obsidian_uri_simple() {
        let uri = build_obsidian_uri("MyVault", "Inbox/example");
        assert_eq!(uri, "obsidian://open?vault=MyVault&file=Inbox/example");
    }

    #[test]
    fn test_build_obsidian_uri_with_spaces() {
        let uri = build_obsidian_uri("My Vault", "Inbox/notes");
        assert!(uri.contains("vault=My%20Vault"));
        assert!(uri.contains("file=Inbox/notes"));
    }

    #[test]
    fn test_build_obsidian_uri_preserves_slashes() {
        let uri = build_obsidian_uri("MyVault", "Folder/Subfolder/note");
        assert!(uri.contains("file=Folder/Subfolder/note"));
        assert!(!uri.contains("%2F"));
    }

    #[test]
    fn test_build_obsidian_uri_encodes_special_chars() {
        let uri = build_obsidian_uri("My&Vault", "note=1");
        assert!(uri.contains("vault=My%26Vault"));
        assert!(uri.contains("file=note%3D1"));
    }

    #[test]
    fn test_extract_vault_name() {
        assert_eq!(
            extract_vault_name(Path::new("/home/user/Obsidian/MyVault")),
            "MyVault"
        );
    }

    #[test]
    fn test_extract_vault_name_single() {
        assert_eq!(extract_vault_name(Path::new("MyVault")), "MyVault");
    }

    #[test]
    fn test_extract_vault_name_empty() {
        assert_eq!(extract_vault_name(Path::new("")), "Unknown");
    }

    #[test]
    fn test_extract_vault_name_root() {
        assert_eq!(extract_vault_name(Path::new("/")), "Unknown");
    }

    #[test]
    fn test_encode_neutralizes_pipe_injection() {
        let uri = build_obsidian_uri("foo|calc.exe", "note");
        assert!(!uri.contains('|'));
        assert!(uri.contains("vault=foo%7Ccalc.exe"));
    }

    #[test]
    fn test_encode_neutralizes_all_cmd_metacharacters() {
        for meta in ['|', '>', '<', '^', ';', '(', ')', '&', '"', '\n', '\r'] {
            let input = format!("a{meta}b");
            let uri = build_obsidian_uri(&input, "note");
            // Isolate the encoded vault value (between `vault=` and `&file=`).
            // The whole URI structurally contains '&' and '=' as query separators,
            // so asserting on the full string would false-positive on those
            // legitimate characters even though the value is correctly encoded.
            let vault_value = uri
                .strip_prefix("obsidian://open?vault=")
                .and_then(|rest| rest.split("&file=").next())
                .unwrap_or_default();
            assert!(
                !vault_value.contains(meta),
                "metacharacter {meta:?} leaked into vault value: {vault_value}"
            );
        }
    }

    #[test]
    fn test_validate_rejects_control_chars() {
        assert!(validate_obsidian_input("vault\nname", "note").is_err());
        assert!(validate_obsidian_input("vault", "note\0path").is_err());
    }

    #[test]
    fn test_validate_accepts_normal_input() {
        assert!(validate_obsidian_input("My Vault", "Folder/Subfolder/note").is_ok());
    }

    /// XP-S-06 (#1608): the bounded wait reports a normal exit.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn wait_for_exit_with_timeout_reports_normal_exit() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let outcome = wait_for_exit_with_timeout(&mut child, std::time::Duration::from_secs(5))
            .and_then(|r| r.ok())
            .expect("true must exit within the bound");
        assert!(outcome.success());
    }

    /// XP-S-06 (#1608): a wedged handler is killed at the deadline and the
    /// wait reports `None` — the dispatch never hangs forever.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn wait_for_exit_with_timeout_kills_a_wedged_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let started = std::time::Instant::now();
        let outcome = wait_for_exit_with_timeout(&mut child, std::time::Duration::from_millis(150));
        assert!(outcome.is_none(), "sleep 30 must hit the 150ms deadline");
        // The kill must have happened at (not long after) the deadline.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "wait must return promptly after killing the child"
        );
    }
}
