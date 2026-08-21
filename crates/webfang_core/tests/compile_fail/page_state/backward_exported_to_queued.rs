//! EXPORTED → QUEUED: backward move outside the recovery rule
//! (`reopen_for_reexport` goes to PROCESSED, never to QUEUED) must not
//! compile.

use std::path::PathBuf;
use webfang_core::domain::page_state::{Stateful};

fn main() {
    let s = Stateful::new(String::from("https://example.com/back"));
    let s = s.queue().start_fetch().fetched().extracted().processed();
    let s = s.export_flushed(PathBuf::from("out/back.jsonl"));
    let _ = s.queue();
}
