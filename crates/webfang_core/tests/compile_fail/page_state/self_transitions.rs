//! Self-transitions (X → X) for every non-terminal state: no method maps a
//! state onto itself, so each call below must not compile.

fn main() {
    // QUEUED → QUEUED
    {
        use webfang_core::domain::page_state::{Queued, Stateful};
        let q: Stateful<String, Queued> = Stateful::new(String::new()).queue();
        let _ = q.queue();
    }
    // FETCHING → FETCHING
    {
        use webfang_core::domain::page_state::{Fetching, Stateful};
        let f: Stateful<String, Fetching> = Stateful::new(String::new()).queue().start_fetch();
        let _ = f.start_fetch();
    }
    // FETCHED → FETCHED
    {
        use webfang_core::domain::page_state::{Fetched, Stateful};
        let f: Stateful<String, Fetched> =
            Stateful::new(String::new()).queue().start_fetch().fetched();
        let _ = f.fetched();
    }
    // EXTRACTED → EXTRACTED
    {
        use webfang_core::domain::page_state::{Extracted, Stateful};
        let e: Stateful<String, Extracted> = Stateful::new(String::new())
            .queue()
            .start_fetch()
            .fetched()
            .extracted();
        let _ = e.extracted();
    }
    // PROCESSED → PROCESSED
    {
        use webfang_core::domain::page_state::{Processed, Stateful};
        let p: Stateful<String, Processed> = Stateful::new(String::new())
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
        let e: Stateful<String, Exported> = Stateful::new(String::new())
            .queue()
            .start_fetch()
            .fetched()
            .extracted()
            .processed()
            .export_flushed(PathBuf::from("out/self.jsonl"));
        let _ = e.export_flushed(PathBuf::from("out/self.jsonl"));
    }
}
