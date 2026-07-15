//! Obsidian URI protocol support.
//!
//! Opens notes directly in Obsidian using the `obsidian://` URI scheme.
//!
//! URI format: `obsidian://open?vault=<vault_name>&file=<file_path>`

use std::path::Path;

/// Minimal encoding for Obsidian URI parameters.
///
/// Unlike full URL encoding, this preserves forward slashes and other
/// characters that Obsidian expects unencoded in URI query values.
/// Only encodes characters that would break URI parsing: `&`, `=`,
/// `#`, `?`, `%`, `+`, space, and non-ASCII.
fn encode_obsidian_param(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' | '=' | '#' | '?' | '%' | '+' => {
                out.push_str(&format!("%{:02X}", ch as u32));
            },
            ' ' => {
                out.push_str("%20");
            },
            c if c.is_ascii() => {
                out.push(c);
            },
            c => {
                // Non-ASCII: UTF-8 percent-encode each byte
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
    let (cmd, args) = if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", uri])
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
}
