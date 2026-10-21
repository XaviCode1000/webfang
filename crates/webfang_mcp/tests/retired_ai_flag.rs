//! The `--enable-ai` flag and its `WEBFANG_MCP_AI` env binding are retired
//! from both MCP binaries (issue #2123).
//!
//! Contract under test — observable only, no network, no server boot:
//! - neither binary's `--help` mentions `--enable-ai`, `WEBFANG_MCP_AI` or
//!   `enable_ai`: the knob is no longer part of the offered surface;
//! - passing `--enable-ai` is a clap USAGE error — non-zero exit, the
//!   rejected argument named on stderr, nothing on stdout. A client that
//!   still sends the flag fails at parse time instead of booting a server
//!   that enabled nothing.
//!
//! Why the rejection is the contract and not a nicety: the flag used to be
//! accepted and silently ignored, so a client could believe the AI wiring was
//! on while the tools answered "not available". Retiring it makes the
//! mismatch loud at the boundary.
//!
//! Run with: cargo nextest run --test retired_ai_flag

use std::process::Command;

/// Both MCP binaries of this crate, by their `[[bin]]` names. The
/// `CARGO_BIN_EXE_*` env vars are set for the owning crate, so they resolve
/// here (a sibling crate's binary would NOT).
const BINARIES: [&str; 2] = ["webfang-mcp", "webfang-mcp-stdio"];

/// Spawn one of the two MCP binaries with the given args.
fn binary(name: &str) -> Command {
    let path = match name {
        "webfang-mcp" => env!("CARGO_BIN_EXE_webfang-mcp"),
        _ => env!("CARGO_BIN_EXE_webfang-mcp-stdio"),
    };
    Command::new(path)
}

#[test]
fn help_never_advertises_the_retired_ai_flag() {
    for name in BINARIES {
        let out = binary(name).arg("--help").output().expect("run --help");
        assert!(
            out.status.success(),
            "{name} --help must exit 0, got {:?}",
            out.status.code()
        );

        let stdout = String::from_utf8_lossy(&out.stdout);
        for forbidden in ["--enable-ai", "WEBFANG_MCP_AI", "enable_ai"] {
            assert!(
                !stdout.contains(forbidden),
                "{name} --help still advertises `{forbidden}`:\n{stdout}"
            );
        }
    }
}

#[test]
fn the_retired_flag_is_a_usage_error_in_both_binaries() {
    for name in BINARIES {
        let out = binary(name)
            .arg("--enable-ai")
            .output()
            .expect("run with --enable-ai");

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);

        assert!(
            !out.status.success(),
            "{name} accepted --enable-ai: it must be rejected at parse time"
        );
        // clap's usage-error exit code (2), not a runtime failure code: this
        // is a parse rejection, and nothing downstream of the parser ran.
        assert_eq!(
            out.status.code(),
            Some(2),
            "{name} must fail with clap's usage code (2):\n{stderr}"
        );
        assert!(
            stderr.contains("--enable-ai"),
            "{name} must name the rejected argument:\n{stderr}"
        );
        assert!(
            stdout.is_empty(),
            "{name} wrote to stdout before failing: the server must not start:\n{stdout}"
        );
    }
}
