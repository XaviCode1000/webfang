//! The torn-tail recovery of a JSONL export (#2237).
//!
//! Recovering a torn JSONL tail needs ONE thing: the LAST newline boundary
//! of the export. Everything before it is already durable, so it is never
//! loaded — the bytes are read through a WINDOW at the end of the artifact,
//! which grows backwards until it reaches the boundary.
//!
//! This module owns that window (its initial size, its explicit cap and the
//! read itself) and the recovery that consumes it (the truncation, the `.bak`
//! and the warnings). It also owns the EVIDENCE that pins both: the unit
//! tests of the recovery and the peak-RSS probe of the tail read.
//!
//! The split is by responsibility: the window is a bounded READ and the
//! recovery is a bounded REWRITE, and only the second one touches the file.
//! The session that opens the file lives in the parent module
//! (`jsonl_writer`); it calls `recover_torn_tail` before it writes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use tracing::warn;

/// Size of the FIRST tail window read by the torn-tail recovery
/// (`recover_torn_tail`, #2237).
///
/// The recovery only needs the bytes between the last newline boundary and
/// the end of the file, so it reads a WINDOW at the end of the artifact
/// instead of the whole artifact. 64 KiB covers the vast majority of export
/// records; a longer one grows the window up to [`MAX_TAIL_WINDOW_BYTES`].
pub const TAIL_WINDOW_BYTES: u64 = 64 * 1024;

/// Hard cap for the tail window read by the torn-tail recovery (#2237).
///
/// A record longer than this makes the recovery FAIL with an explicit error:
/// cutting the tail without ever seeing its newline boundary would report a
/// clean recovery while destroying bytes nobody inspected (a false green).
pub const MAX_TAIL_WINDOW_BYTES: u64 = 8 * 1024 * 1024;

/// Cut an invalid partial last line back to the last newline boundary.
///
/// Bytes after the final newline never got their terminator, so they cannot be
/// a durable line regardless of content — truncate them and say how many.
///
/// M3 (#1617): truncation is a destructive rewrite, and the `None` branch
/// below wipes the WHOLE file, so both branches back up the pre-truncation
/// bytes first. Before this, a torn tail was the one destructive rewrite in
/// the export pipeline with no recovery at all: once the file was reopened for
/// append, the truncated records were gone.
///
/// #2237: the file is NOT read whole. Only the TAIL is read, through a
/// bounded window ([`TAIL_WINDOW_BYTES`], growing up to
/// [`MAX_TAIL_WINDOW_BYTES`]): the recovery needs the last newline boundary,
/// so the bytes before it are never loaded. The peak no longer grows with the
/// size of the accumulated export.
///
/// The backed-up bytes are the ABANDONED ones (the tail that is about to be
/// lost), not the whole file: the surviving prefix stays in the live file, so
/// the backup holds exactly the bytes the truncation destroys.
pub(crate) fn recover_torn_tail(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let file_len = fs::metadata(path)?.len();
    if file_len == 0 {
        return Ok(());
    }
    let (window, window_start) = read_tail_window(path, file_len)?;
    match window.iter().rposition(|&b| b == b'\n') {
        Some(last_newline) => {
            let boundary = window_start + last_newline as u64 + 1;
            let trailing = file_len - boundary;
            if trailing > 0 {
                crate::application::resume::preserve_abandoned_bytes(
                    path,
                    &window[last_newline + 1..],
                    "jsonl_torn_tail_truncated",
                );
                let f = OpenOptions::new().write(true).open(path)?;
                f.set_len(boundary)?;
                warn!(
                    truncated_bytes = trailing,
                    total_bytes = file_len,
                    "torn JSONL tail truncated back to the last valid newline"
                );
            }
        },
        None => {
            // No newline at all: one unterminated line that cannot be durable
            // — unterminated means not durable. The window covered the WHOLE
            // file here (a file with no newline that does not fit in the
            // window is rejected by `read_tail_window`), so every byte of the
            // window is abandoned.
            crate::application::resume::preserve_abandoned_bytes(
                path,
                &window,
                "jsonl_unterminated_file_wiped",
            );
            fs::write(path, b"")?;
            warn!(
                truncated_bytes = file_len,
                "unterminated JSONL line truncated: file had no newline boundary"
            );
        },
    }
    Ok(())
}

/// Read the tail window that holds the LAST newline boundary of the file.
///
/// Returns the window bytes together with the absolute offset of its first
/// byte, so a newline found inside the window can be mapped back to a file
/// position. The window starts at [`TAIL_WINDOW_BYTES`] and DOUBLES towards
/// the beginning of the file until it reaches a newline or covers the whole
/// file.
///
/// # Errors
///
/// Fails when the window reaches [`MAX_TAIL_WINDOW_BYTES`] without finding a
/// newline boundary: the tail is then longer than the recovery can inspect,
/// and truncating it blind would destroy bytes nobody saw (a false green).
pub(crate) fn read_tail_window(path: &Path, file_len: u64) -> io::Result<(Vec<u8>, u64)> {
    let mut window = TAIL_WINDOW_BYTES.min(file_len);
    loop {
        let start = file_len - window;
        let bytes = read_range(path, start, window)?;
        if bytes.contains(&b'\n') || start == 0 {
            return Ok((bytes, start));
        }
        if window >= MAX_TAIL_WINDOW_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "torn JSONL tail is longer than the recovery window: no newline boundary \
                     in the last {window} bytes of a {file_len}-byte file, and the \
                     {MAX_TAIL_WINDOW_BYTES}-byte window cap was reached"
                ),
            ));
        }
        window = (window * 2).min(MAX_TAIL_WINDOW_BYTES).min(file_len);
    }
}

/// Read `len` bytes of `path` starting at `offset`.
pub(crate) fn read_range(path: &Path, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let len = usize::try_from(len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "tail window too large"))?;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::super::JsonlSession;
    use super::{read_tail_window, recover_torn_tail, MAX_TAIL_WINDOW_BYTES, TAIL_WINDOW_BYTES};
    use std::collections::HashSet;
    use std::fs;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;

    fn valid_line(n: u32) -> String {
        format!(r#"{{"n":{n},"checksum_sha256":"hash-{n}"}}"#)
    }

    fn terminated(line: &str) -> Vec<u8> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        bytes
    }

    // ------------------------------------------------------------------
    // Torn-tail recovery (SC: crash mid-write leaves half-written line)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn torn_tail_is_truncated_to_last_valid_newline_before_appends() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("torn.jsonl");

        let mut initial = String::new();
        initial.push_str(&valid_line(1));
        initial.push('\n');
        initial.push_str(&valid_line(2));
        initial.push('\n');
        initial.push_str(r#"{"n":3,"checksum_sha256":"ha"#); // torn tail, no newline
        fs::write(&path, &initial).expect("seed torn file");

        let (session, index) = JsonlSession::open(&path).expect("open recovers torn tail");

        // Truncation happened synchronously at open time, BEFORE appends.
        let recovered = fs::read_to_string(&path).expect("read recovered file");
        assert_eq!(
            recovered,
            format!("{}\n{}\n", valid_line(1), valid_line(2)),
            "tail must be cut back to the last valid newline"
        );
        assert!(
            index.contains("hash-1") && index.contains("hash-2") && !index.contains("hash-3"),
            "hash index must reflect exactly the surviving valid lines"
        );

        session
            .append(&terminated(&valid_line(3)))
            .await
            .expect("append after recovery");
        session.close().await.expect("clean exit");

        let final_content = fs::read_to_string(&path).expect("final read");
        let lines: Vec<&str> = final_content.lines().collect();
        assert_eq!(lines.len(), 3, "appends start at the newline boundary");
        assert!(serde_json::from_str::<serde_json::Value>(lines[2]).is_ok());
        // The recovered prefix is byte-identical to the two surviving lines,
        // and the re-appended line starts exactly at that boundary.
        let expected = format!("{}\n{}\n{}\n", valid_line(1), valid_line(2), valid_line(3));
        assert_eq!(final_content, expected);
    }

    #[tokio::test]
    async fn clean_file_is_not_modified_by_open_scan() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("clean.jsonl");
        let pristine = format!("{}\n{}\n", valid_line(1), valid_line(2));
        fs::write(&path, &pristine).expect("seed clean file");

        let (_session, index) = JsonlSession::open(&path).expect("open clean file");

        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            pristine,
            "a fully valid file must not be touched by recovery"
        );
        assert!(
            !crate::application::resume::backup_sibling(&path).exists(),
            "a clean file must not produce a .bak — only a destructive rewrite backs up"
        );
        assert_eq!(
            index,
            HashSet::from(["hash-1".to_string(), "hash-2".to_string()])
        );
    }

    /// M3 (#1617): truncating a torn tail destroys bytes. The `.bak` is the
    /// only way back to the pre-truncation file once the output is reopened
    /// for append, so recovery must not be a one-way door. Also pins that the
    /// backup keeps the artifact's own extension (`out.jsonl.bak`).
    ///
    /// #2237 (S3): the backup carries the ABANDONED bytes (the tail the
    /// truncation destroys), not the whole file: the surviving prefix stays in
    /// the live file, so prefix + `.bak` reconstructs the pre-truncation file
    /// without the backup duplicating the bytes that were never at risk.
    #[tokio::test]
    async fn torn_tail_recovery_backs_up_the_abandoned_bytes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("out.jsonl");
        let prefix = format!("{}\n", valid_line(1));
        let abandoned = r#"{"n":2,"checksum_sha256":"partial"#;
        fs::write(&path, format!("{prefix}{abandoned}")).expect("seed torn file");

        let (_session, _index) = JsonlSession::open(&path).expect("open recovers torn tail");

        let backup = crate::application::resume::backup_sibling(&path);
        assert_eq!(
            backup.file_name().and_then(|n| n.to_str()),
            Some("out.jsonl.bak"),
            "the backup keeps the artifact's own extension"
        );
        assert_eq!(
            fs::read_to_string(&backup).expect("read backup"),
            abandoned,
            "the .bak must carry the exact abandoned bytes (the truncated tail)"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read recovered"),
            prefix,
            "the live file is still truncated back to the last newline"
        );
    }

    /// #2237 (S2): a torn tail LONGER than the initial window must still be
    /// recovered: the window grows backwards until it reaches the newline
    /// boundary, and the same bytes are truncated.
    ///
    /// The same coverage runs through the PUBLIC API in
    /// `tests/export_torn_tail_recovery_test.rs`.
    #[tokio::test]
    async fn torn_tail_longer_than_the_initial_window_grows_it_until_the_newline() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("long.jsonl");
        let prefix = format!("{}\n", valid_line(1));
        // Three windows: the first read cannot see the newline boundary.
        let abandoned = "x".repeat((TAIL_WINDOW_BYTES * 3) as usize);
        fs::write(&path, format!("{prefix}{abandoned}")).expect("seed long torn file");

        let (session, _index) = JsonlSession::open(&path).expect("open recovers long torn tail");
        session.close().await.expect("clean exit");

        assert_eq!(
            fs::read_to_string(&path).expect("read recovered"),
            prefix,
            "the long tail is cut back to the last newline"
        );
        assert_eq!(
            fs::read_to_string(crate::application::resume::backup_sibling(&path))
                .expect("read backup"),
            abandoned,
            "the abandoned bytes are preserved whole, however long the tail is"
        );
    }

    /// #2237 (S4): a file with NO newline at all is a single unterminated
    /// line: it cannot be durable, so it is wiped — and the whole file (which
    /// is, in this case, the abandoned tail) is preserved.
    ///
    /// The same coverage runs through the PUBLIC API in
    /// `tests/export_torn_tail_recovery_test.rs`.
    #[tokio::test]
    async fn unterminated_file_without_any_newline_is_wiped_and_backed_up() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("unterminated.jsonl");
        let content = r#"{"n":1,"checksum_sha256":"hash-1""#;
        fs::write(&path, content).expect("seed unterminated file");

        let (session, index) = JsonlSession::open(&path).expect("open wipes unterminated file");

        assert!(
            fs::read(&path).expect("read recovered").is_empty(),
            "a file with no newline boundary has no durable line"
        );
        assert_eq!(
            fs::read_to_string(crate::application::resume::backup_sibling(&path))
                .expect("read backup"),
            content,
            "the abandoned bytes are the whole file here"
        );
        assert!(
            index.is_empty(),
            "no line survives, so the hash index is empty"
        );
        session.close().await.expect("clean exit");
    }

    /// #2237 (S2): the window has an EXPLICIT cap. A tail longer than the cap
    /// is not truncated blind: the recovery fails with an error that names the
    /// cause, and the file is left untouched (nothing is destroyed by a
    /// recovery that could not see the boundary).
    #[tokio::test]
    async fn torn_tail_beyond_the_window_cap_fails_without_destroying_anything() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("beyond-cap.jsonl");
        let content = vec![b'x'; (MAX_TAIL_WINDOW_BYTES + 1) as usize];
        fs::write(&path, &content).expect("seed oversized unterminated file");

        let error = JsonlSession::open(&path).expect_err("recovery must refuse a blind cut");

        let message = error.to_string();
        assert!(
            message.contains("window") && message.contains(&MAX_TAIL_WINDOW_BYTES.to_string()),
            "the error must name the window cap that was reached: {message}"
        );
        assert_eq!(
            fs::metadata(&path).expect("stat").len(),
            content.len() as u64,
            "a refused recovery must not truncate the file"
        );
        assert!(
            !crate::application::resume::backup_sibling(&path).exists(),
            "a refused recovery must not write a backup of bytes it did not abandon"
        );
    }

    // ------------------------------------------------------------------
    // #2237 (T2): peak-RSS probe of the tail read
    // ------------------------------------------------------------------

    /// Env var that turns this test into the CHILD probe process.
    const PROBE_MODE_ENV: &str = "WEBFANG_TAIL_PROBE_MODE";
    /// Env var carrying the artifact the child probe must read.
    const PROBE_FILE_ENV: &str = "WEBFANG_TAIL_PROBE_FILE";

    /// Peak-RSS probe of the tail read (#2237, T2).
    ///
    /// IGNORED by default: it writes a ~69 MB artifact and forks one child
    /// process per strategy (this same test binary, re-executed) to compare
    /// the peak RSS of the OLD strategy (`fs::read` of the WHOLE file) against
    /// the NEW one (the windowed tail read) over the SAME artifact.
    ///
    /// Run it explicitly:
    ///
    /// ```text
    /// cargo test -p webfang_core --lib -- --ignored --nocapture tail_read_memory_probe
    /// ```
    #[test]
    #[ignore = "T2 measurement: writes a ~69 MB artifact and forks two probe processes"]
    fn tail_read_memory_probe() {
        match std::env::var(PROBE_MODE_ENV).ok().as_deref() {
            Some("control") => probe_child("control"),
            Some("window") => probe_child("window"),
            Some("idle") => probe_child("idle"),
            Some("read_only") => probe_child("read_only"),
            Some("io_only") => probe_child("io_only"),
            Some(other) => panic!("unknown probe mode: {other}"),
            None => probe_parent(),
        }
    }

    /// Child process: measure the peak RSS of ONE strategy over the artifact.
    fn probe_child(mode: &str) {
        let path = std::path::PathBuf::from(std::env::var(PROBE_FILE_ENV).expect("probe file env"));
        let rss_before = status_kib("VmRSS");
        let hwm_before = status_kib("VmHWM");
        match mode {
            // The OLD strategy: the WHOLE file in memory.
            "control" => {
                let data = fs::read(&path).expect("control read");
                std::hint::black_box(&data);
            },
            // The NEW strategy: only the tail window.
            "window" => {
                recover_torn_tail(&path).expect("window recovery");
            },
            // Only the window read, without the destructive rewrite.
            "read_only" => {
                let len = fs::metadata(&path).expect("stat").len();
                let (bytes, start) = read_tail_window(&path, len).expect("window read");
                std::hint::black_box((&bytes, start));
            },
            // The plain I/O floor: open the file and read a single byte.
            "io_only" => {
                let mut file = File::open(&path).expect("open");
                let mut byte = [0u8; 1];
                file.read_exact(&mut byte).expect("read one byte");
                std::hint::black_box(byte);
            },
            // The harness itself: the floor every mode pays.
            _ => {},
        }
        let hwm_after = status_kib("VmHWM");
        let rss_after = status_kib("VmRSS");
        println!(
            "TAIL_PROBE mode={mode} rss_before={rss_before} hwm_before={hwm_before} \
             rss_after={rss_after} hwm_after={hwm_after}"
        );
    }

    /// Parent process: build the artifact, run one probe per strategy, compare.
    fn probe_parent() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let template = dir.path().join("template.jsonl");
        let size = write_probe_artifact(&template);
        let exe = std::env::current_exe().expect("test binary path");

        let mut results = Vec::new();
        for mode in ["idle", "io_only", "read_only", "control", "window"] {
            let target = dir.path().join(format!("{mode}.jsonl"));
            fs::copy(&template, &target).expect("copy artifact");
            let output = std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "infrastructure::export::jsonl_writer::tail_window::tests::tail_read_memory_probe",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(PROBE_MODE_ENV, mode)
                .env(PROBE_FILE_ENV, &target)
                .output()
                .expect("probe child runs");
            let stdout = String::from_utf8_lossy(&output.stdout);
            // The libtest harness interleaves the test's own stdout with the
            // test name on the SAME line, so the marker is not at the line
            // start: cut the line from the marker on.
            let line = stdout
                .split_once("TAIL_PROBE ")
                .map(|(_, rest)| {
                    rest.split_once('\n')
                        .map_or(rest, |(head, _)| head)
                        .to_string()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no probe result (stdout: {stdout}) (stderr: {})",
                        String::from_utf8_lossy(&output.stderr)
                    )
                });
            println!("{line}");
            results.push(line);
        }

        let idle = &results[0];
        let io_only = &results[1];
        let read_only = &results[2];
        let control = &results[3];
        let window = &results[4];
        let idle_growth = probe_growth_kib(idle);
        let io_growth = probe_growth_kib(io_only);
        let read_growth = probe_growth_kib(read_only);
        let control_growth = probe_growth_kib(control);
        let window_growth = probe_growth_kib(window);
        println!(
            "TAIL_PROBE_SUMMARY artifact_bytes={size} idle_growth_kib={idle_growth} \
             io_only_growth_kib={io_growth} read_only_growth_kib={read_growth} \
             control_growth_kib={control_growth} window_growth_kib={window_growth} \
             control_minus_idle_kib={} window_minus_idle_kib={}",
            control_growth.saturating_sub(idle_growth),
            window_growth.saturating_sub(idle_growth)
        );
        assert!(
            control_growth > 32 * 1024,
            "the control (whole file in memory) must show a large peak: {control_growth} KiB"
        );
        assert!(
            window_growth < 8 * 1024,
            "the windowed read must stay under the window cap: {window_growth} KiB"
        );
    }

    /// Peak growth (KiB) of a probe line: peak RSS after minus RSS before.
    fn probe_growth_kib(line: &str) -> u64 {
        let rss_before = probe_field(line, "rss_before");
        let hwm_after = probe_field(line, "hwm_after");
        hwm_after.saturating_sub(rss_before)
    }

    /// Read one `key=value` number out of a probe line.
    fn probe_field(line: &str, key: &str) -> u64 {
        let needle = format!("{key}=");
        let start = line
            .find(&needle)
            .unwrap_or_else(|| panic!("{key} missing from {line}"))
            + needle.len();
        let value: String = line[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        value
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("{key} is not a number in {line}"))
    }

    /// A field of `/proc/self/status` in KiB (0 when the field is absent).
    fn status_kib(field: &str) -> u64 {
        let Ok(status) = fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix(field) {
                let rest = rest.trim_start_matches(':').trim();
                let value = rest.split_whitespace().next().unwrap_or("0");
                return value.parse::<u64>().unwrap_or(0);
            }
        }
        0
    }

    /// Write the probe artifact: complete lines plus a torn tail, at the size
    /// of the reference export measured for #2237.
    fn write_probe_artifact(path: &Path) -> u64 {
        const TARGET: u64 = 69_244_058;
        const TAIL: &[u8] = b"{\"n\":999999999,\"checksum_sha256\":\"torn";
        const LINE: &[u8] = b"{\"n\":1,\"url\":\"https://example.com/page\",\"content\":\"lorem ipsum dolor sit amet\",\"checksum_sha256\":\"0123456789abcdef0123456789abcdef\"}\n";
        let body = TARGET - TAIL.len() as u64 - 1;
        let mut file = fs::File::create(path).expect("create artifact");
        let mut chunk = Vec::with_capacity(1 << 20);
        let mut written = 0u64;
        while written + LINE.len() as u64 <= body {
            chunk.extend_from_slice(LINE);
            written += LINE.len() as u64;
            if chunk.len() >= (1 << 20) {
                file.write_all(&chunk).expect("write chunk");
                chunk.clear();
            }
        }
        file.write_all(&chunk).expect("write chunk");
        let remaining = body - written;
        if remaining > 0 {
            file.write_all(&vec![b'x'; remaining as usize])
                .expect("write padding");
        }
        file.write_all(b"\n").expect("write newline");
        file.write_all(TAIL).expect("write torn tail");
        file.flush().expect("flush artifact");
        fs::metadata(path).expect("stat artifact").len()
    }
}
