//! COMMITTED → anything: COMMITTED is terminal in the type system; no
//! transition method exists on `Stateful<_, Committed>`.

use std::path::PathBuf;
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
    let s = Stateful::new(rec("https://example.com/done"))
        .queue()
        .start_fetch()
        .fetched()
        .extracted()
        .processed();
    #[allow(path_statements)]
    {
        let committed =
            s.export_flushed(PathBuf::from("out/done.jsonl")).commit();
        let _ = committed.queue();
        let _ = committed.start_fetch();
        let _ = committed.reopen_for_reexport();
    }
}
