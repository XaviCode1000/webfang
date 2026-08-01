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
/// Shell metacharacters are neutralized by [`encode_obsidian_param`] (percent-encoded),
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

/// Open a note in Obsidian using the URI protocol (fire-and-forget).
///
/// Uses `xdg-open` on Linux, `open` on macOS, `start` on Windows.
/// Non-blocking: spawns the process and returns immediately.
///
/// # Arguments
/// - `uri` — The obsidian:// URI to open
///
/// # Returns
/// `Ok(())` on spawn, `Err(String)` if command fails to start
pub fn open_in_obsidian(uri: &str) -> Result<(), String> {
    // The URI is fully percent-encoded by `build_obsidian_uri` (no raw
    // metacharacters or quotes can appear), and on Windows the empty `""`
    // title prevents `start` from consuming the URI as a window title.
    // Together these make `cmd /C start` safe on Windows.
    let (cmd, args) = if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", uri])
    } else if cfg!(target_os = "macos") {
        ("open", vec![uri])
    } else {
        // Linux: use xdg-open (standard on all Linux desktops)
        ("xdg-open", vec![uri])
    };

    // Fire-and-forget: spawn and don't wait
    std::process::Command::new(cmd)
        .args(&args)
        .spawn()
        .map_err(|e| format!("failed to open URI: {e}"))?;

    Ok(())
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
/// `Ok(())` if URI was opened (or spawned), `Err(String)` on failure
pub fn open_note(vault_path: &Path, file_path: &Path) -> Result<(), String> {
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
}
