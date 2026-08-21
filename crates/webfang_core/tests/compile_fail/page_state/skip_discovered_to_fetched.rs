//! DISCOVERED → FETCHED state skip: `fetched` is only defined on the
//! FETCHING source impl, so calling it on DISCOVERED must not compile.

use webfang_core::domain::page_state::{PageStatus, PersistedRecord};

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

use webfang_core::domain::page_state::Stateful;

fn main() {
    let s = Stateful::new(rec("https://example.com/skip"));
    let _ = s.fetched();
}
