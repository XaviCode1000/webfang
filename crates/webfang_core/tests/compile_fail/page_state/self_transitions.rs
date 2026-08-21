//! Self-transitions (X → X) for every non-terminal state: no method maps a
//! state onto itself, so each call below must not compile.

use webfang_core::domain::page_state::{PageStatus, PersistedRecord, Stateful};

#[derive(Clone)]
struct Rec {
    #[allow(dead_code)]
    url: String,
    status: PageStatus,
}

impl PersistedRecord for Rec {
    fn status(&self) -> PageStatus {
        self.status
    }

    fn output_location(&self) -> Option<&str> {
        None
    }

    fn content_hash(&self) -> Option<&str> {
        None
    }

    fn has_last_error(&self) -> bool {
        false
    }

    fn attempts(&self) -> u32 {
        0
    }

    fn set_status(&mut self, status: PageStatus) {
        self.status = status;
    }
}

fn rec(url: &str) -> Rec {
    Rec {
        url: url.to_string(),
        status: PageStatus::Discovered,
    }
}

fn main() {
    // QUEUED → QUEUED
    {
        use webfang_core::domain::page_state::{Queued, Stateful};
        let q: Stateful<Rec, Queued> = Stateful::new(rec("https://example.com/self")).queue();
        let _ = q.queue();
    }
    // FETCHING → FETCHING
    {
        use webfang_core::domain::page_state::{Fetching, Stateful};
        let f: Stateful<Rec, Fetching> = Stateful::new(rec("https://example.com/self")).queue().start_fetch();
        let _ = f.start_fetch();
    }
    // FETCHED → FETCHED
    {
        use webfang_core::domain::page_state::{Fetched, Stateful};
        let f: Stateful<Rec, Fetched> =
            Stateful::new(rec("https://example.com/self")).queue().start_fetch().fetched();
        let _ = f.fetched();
    }
    // EXTRACTED → EXTRACTED
    {
        use webfang_core::domain::page_state::{Extracted, Stateful};
        let e: Stateful<Rec, Extracted> = Stateful::new(rec("https://example.com/self"))
            .queue()
            .start_fetch()
            .fetched()
            .extracted();
        let _ = e.extracted();
    }
    // PROCESSED → PROCESSED
    {
        use webfang_core::domain::page_state::{Processed, Stateful};
        let p: Stateful<Rec, Processed> = Stateful::new(rec("https://example.com/self"))
            .queue()
            .start_fetch()
            .fetched()
            .extracted()
            .processed();
        let _ = p.processed();
    }
    // EXPORTED → EXPORTED
    {
        use std::path::PathBuf;
        use webfang_core::domain::page_state::{Exported, Stateful};
        let e: Stateful<Rec, Exported> = Stateful::new(rec("https://example.com/self"))
            .queue()
            .start_fetch()
            .fetched()
            .extracted()
            .processed()
            .export_flushed(PathBuf::from("out/self.jsonl"));
        let _ = e.export_flushed(PathBuf::from("out/self.jsonl"));
    }
}
