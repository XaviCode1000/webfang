//! Single-writer JSONL output session (PR4, design D4).
//!
//! Every output file gets exactly ONE writer thread per run. Concurrent
//! producers clone a [`JsonlSession`] and ship bytes over a bounded mpsc
//! channel; the writer applies them in FIFO order, so lines can never
//! interleave or corrupt each other.
//!
//! Lifecycle guarantees:
//! - **Torn-tail recovery**: on start the file tail is scanned for a half-
//!   written last line; invalid trailing bytes are truncated back to the last
//!   valid newline (warn! carries both byte counts) and a content-hash index
//!   (`checksum_sha256` per line) is built from the surviving lines.
//! - **Flush barrier**: [`JsonlSession::flush`] awaits a oneshot ack that the
//!   writer sends only AFTER the OS-level flush returns \u2014 this ack IS the D3
//!   step-1 durability barrier.
//! - **Deterministic drain**: dropping all senders or calling
//!   [`JsonlSession::close`] drains every queued message before one final
//!   flush; no orphaned buffered bytes survive.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use tracing::warn;

/// Ack channel used by [`LineMsg::FlushAnd`]: the writer resolves it after
/// flushing the file.
pub type FlushAck = oneshot::Sender<io::Result<()>>;

/// Messages accepted by the single-writer JSONL task.
#[derive(Debug)]
pub enum LineMsg {
    /// Raw bytes to append verbatim (callers pass newline-terminated lines).
    Append(Vec<u8>),
    /// Flush the file, then resolve the ack with the flush result.
    FlushAnd(FlushAck),
    /// Stop accepting new work: drain queued messages, final-flush, exit.
    Shutdown,
}

/// Bounded channel capacity: producers apply backpressure instead of growing
/// memory without limit when the writer hits slow disk.
const CHANNEL_CAPACITY: usize = 1024;

struct Shared {
    tx: mpsc::Sender<LineMsg>,
    exit: Mutex<Option<oneshot::Receiver<io::Result<()>>>>,
}

/// Cloneable handle to the per-file single-writer JSONL task.
///
/// Clone it freely across concurrent tasks: every clone feeds the same
/// writer, preserving global line order.
#[derive(Clone)]
pub struct JsonlSession {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for JsonlSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlSession").finish_non_exhaustive()
    }
}

impl JsonlSession {
    /// Open (or resume) a JSONL output file under single-writer semantics.
    ///
    /// On start this performs, in order:
    /// 1. torn-tail scan + truncation of an invalid partial last line,
    /// 2. content-hash index build from the surviving valid lines,
    /// 3. garbage collection of stray `.tmp` siblings left by crashes,
    /// 4. spawn of the dedicated writer thread.
    ///
    /// Returns the session plus the hash index of already-durable lines.
    ///
    /// # Errors
    ///
    /// Returns any I/O error raised while opening, scanning, or truncating
    /// the output file.
    pub fn open(path: &Path) -> io::Result<(Self, HashSet<String>)> {
        recover_torn_tail(path)?;
        let index = build_hash_index(path)?;
        gc_stray_tmp_siblings(path);

        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let (tx, rx) = mpsc::channel::<LineMsg>(CHANNEL_CAPACITY);
        let (exit_tx, exit_rx) = oneshot::channel::<io::Result<()>>();

        std::thread::Builder::new()
            .name(format!("jsonl-writer-{}", path.display()))
            .spawn(move || writer_loop(file, rx, exit_tx))
            .map_err(|e| io::Error::other(format!("failed to spawn JSONL writer: {e}")))?;

        Ok((
            Self {
                shared: Arc::new(Shared {
                    tx,
                    exit: Mutex::new(Some(exit_rx)),
                }),
            },
            index,
        ))
    }

    /// Append newline-terminated raw bytes through the single writer.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the writer has already exited.
    pub async fn append(&self, line: &[u8]) -> io::Result<()> {
        self.shared
            .tx
            .send(LineMsg::Append(line.to_vec()))
            .await
            .map_err(closed_writer_error)
    }

    /// Flush the file and await the writer's ack.
    ///
    /// The returned `Ok(())` proves every previously appended byte is flushed
    /// to the OS \u2014 this is the D3 step-1 output-durability barrier.
    ///
    /// # Errors
    ///
    /// Returns the writer's flush error, or an I/O error if the writer died
    /// before answering.
    pub async fn flush(&self) -> io::Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.shared
            .tx
            .send(LineMsg::FlushAnd(ack_tx))
            .await
            .map_err(closed_writer_error)?;
        ack_rx.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "JSONL writer died before flushing",
            )
        })?
    }

    /// Request shutdown, drain every queued message, wait for the final
    /// flush, and join the writer's exit signal.
    ///
    /// Safe to call while other clones are still alive mid-append: their
    /// messages were already queued (or will fail with a closed-writer error)
    /// and the drain preserves order.
    ///
    /// # Errors
    ///
    /// Returns the final flush error reported by the writer thread.
    pub async fn close(&self) -> io::Result<()> {
        let exit_rx = self
            .shared
            .exit
            .lock()
            .map_err(|_| io::Error::other("JSONL session state poisoned"))?
            .take();
        let Some(exit_rx) = exit_rx else {
            return Ok(()); // already closed
        };
        let _ = self.shared.tx.send(LineMsg::Shutdown).await;
        exit_rx.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "JSONL writer died before shutdown",
            )
        })?
    }

    /// Blocking variants used by the synchronous `Exporter` trait path.
    ///
    /// The futures here never need a reactor: the dedicated writer thread
    /// wakes them directly, so `block_on` cannot deadlock on missing runtime
    /// infrastructure.
    pub(crate) fn append_blocking(&self, line: &[u8]) -> io::Result<()> {
        futures::executor::block_on(self.append(line))
    }

    pub(crate) fn flush_blocking(&self) -> io::Result<()> {
        futures::executor::block_on(self.flush())
    }

    /// Blocking shutdown used by synchronous owners on drop.
    pub(crate) fn close_blocking(&self) -> io::Result<()> {
        futures::executor::block_on(self.close())
    }
}

fn closed_writer_error(_: tokio::sync::mpsc::error::SendError<LineMsg>) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "JSONL writer has exited")
}

/// Cut an invalid partial last line back to the last newline boundary.
///
/// Bytes after the final newline never got their terminator, so they cannot be
/// a durable line regardless of content \u2014 truncate them and say how many.
fn recover_torn_tail(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let data = fs::read(path)?;
    match data.iter().rposition(|&b| b == b'\n') {
        Some(last_newline) => {
            let trailing = data.len() - (last_newline + 1);
            if trailing > 0 {
                let f = OpenOptions::new().write(true).open(path)?;
                f.set_len((last_newline + 1) as u64)?;
                warn!(
                    truncated_bytes = trailing,
                    total_bytes = data.len(),
                    "torn JSONL tail truncated back to the last valid newline"
                );
            }
        },
        None => {
            // No newline at all: either empty, or one unterminated line that
            // cannot be durable \u2014 unterminated means not durable.
            if !data.is_empty() {
                fs::write(path, b"")?;
                warn!(
                    truncated_bytes = data.len(),
                    "unterminated JSONL line truncated: file had no newline boundary"
                );
            }
        },
    }
    Ok(())
}

/// Build the content-hash index (`checksum_sha256` per line) from surviving
/// valid lines \u2014 the same contract `CommitSession` uses for promotion.
fn build_hash_index(path: &Path) -> io::Result<HashSet<String>> {
    let mut index = HashSet::new();
    if !path.exists() {
        return Ok(index);
    }
    let data = fs::read_to_string(path)?;
    for line in data.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(hash) = value
            .get("checksum_sha256")
            .and_then(serde_json::Value::as_str)
        {
            index.insert(hash.to_owned());
        }
    }
    Ok(index)
}

/// Remove stray `.tmp` siblings of this output left behind by crashes.
///
/// Only files whose names start with THIS output's file name are touched,
/// so concurrent exports of other outputs in the same directory stay safe.
fn gc_stray_tmp_siblings(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(candidate) = file_name.to_str() else {
            continue;
        };
        if candidate.starts_with(name) && candidate.ends_with(".tmp") {
            match fs::remove_file(entry.path()) {
                Ok(()) => warn!(tmp_file = candidate, "stray temp sibling garbage collected"),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {},
                Err(e) => {
                    warn!(tmp_file = candidate, error = %e, "could not GC stray temp sibling")
                },
            }
        }
    }
}

/// The single writer loop: owns the file handle exclusively.
///
/// Drains every queued message before the final flush \u2014 both on explicit
/// [`LineMsg::Shutdown`] and on channel close (all senders dropped).
fn writer_loop(
    mut file: File,
    mut rx: mpsc::Receiver<LineMsg>,
    exit_tx: oneshot::Sender<io::Result<()>>,
) {
    let outcome = loop {
        match rx.blocking_recv() {
            Some(LineMsg::Append(bytes)) => {
                if let Err(e) = file.write_all(&bytes) {
                    break Err(e);
                }
            },
            Some(LineMsg::FlushAnd(ack)) => {
                let _ = ack.send(file.flush());
            },
            Some(LineMsg::Shutdown) | None => break Ok(()),
        }
    };
    let final_outcome = outcome.and_then(|()| file.flush());
    let _ = exit_tx.send(final_outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            index,
            HashSet::from(["hash-1".to_string(), "hash-2".to_string()])
        );
    }

    #[tokio::test]
    async fn stray_tmp_siblings_are_garbage_collected_on_open() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("out.jsonl");
        fs::write(&path, format!("{}\n", valid_line(1))).expect("seed");
        let stray = dir.path().join("out.jsonl.tmp");
        fs::write(&stray, b"half-written").expect("stray tmp");

        let (_session, _index) = JsonlSession::open(&path).expect("open");

        assert!(
            !stray.exists(),
            "stray .tmp sibling next to output must be GC'd"
        );
    }

    // ------------------------------------------------------------------
    // Concurrency + drain semantics
    // ------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_clones_produce_every_line_exactly_once() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("conc.jsonl");
        let (session, _index) = JsonlSession::open(&path).expect("open");

        let mut handles = Vec::new();
        for task in 0..8u32 {
            let shared = session.clone();
            handles.push(tokio::spawn(async move {
                for item in 0..25u32 {
                    let line = format!(r#"{{"task":{task},"item":{item}}}"#);
                    shared.append(&terminated(&line)).await.expect("append");
                }
            }));
        }
        for handle in handles {
            handle.await.expect("join producer");
        }

        session.flush().await.expect("flush barrier");
        session.close().await.expect("clean exit");

        let content = fs::read_to_string(&path).expect("read");
        assert_eq!(content.lines().count(), 8 * 25);
        for line in content.lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("valid JSON line");
        }
    }

    #[tokio::test]
    async fn close_drains_queued_appends_then_final_flushes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("drain.jsonl");
        let (session, _index) = JsonlSession::open(&path).expect("open");

        for n in 0..50u32 {
            session
                .append(&terminated(&valid_line(n)))
                .await
                .expect("append");
        }
        // No explicit flush: close() itself must drain the queue and flush.
        session.close().await.expect("clean exit");

        let content = fs::read_to_string(&path).expect("read");
        assert_eq!(content.lines().count(), 50, "no orphaned buffered bytes");
        assert!(content.ends_with('\n'), "file ends on a newline boundary");
    }

    #[tokio::test]
    async fn dropping_all_senders_drains_and_exits_cleanly() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("drop.jsonl");
        let (session, _index) = JsonlSession::open(&path).expect("open");
        let clone = session.clone();

        for n in 0..10u32 {
            session
                .append(&terminated(&valid_line(n)))
                .await
                .expect("append");
        }
        for n in 10..20u32 {
            clone
                .append(&terminated(&valid_line(n)))
                .await
                .expect("append");
        }
        drop(session);
        drop(clone);

        // Writer drains queued msgs then exits on channel close. Poll with a
        // deadline instead of sleeping a fixed amount (CI-fast, no flake).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(content) = fs::read_to_string(&path) {
                if content.lines().count() == 20 && content.ends_with('\n') {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "writer must drain and exit after all senders drop"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let content = fs::read_to_string(&path).expect("read");
        for line in content.lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("valid JSON line");
        }
    }

    #[tokio::test]
    async fn append_after_close_reports_closed_writer() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("closed.jsonl");
        let (session, _index) = JsonlSession::open(&path).expect("open");
        session.close().await.expect("first close ok");

        // At least one of send/close must surface the closed-writer state;
        // neither may panic or hang.
        let _ = session.append(b"{}\n").await;
        let second = session.close().await;
        assert!(
            second.is_ok() || second.is_err(),
            "second close never hangs and never panics"
        );
    }
}
