//! COMMITTED → anything: COMMITTED is terminal in the type system; no
//! transition method exists on `Stateful<_, Committed>`.

use webfang_core::domain::page_state::{Stateful};

fn main() {
    let s = Stateful::new(String::from("https://example.com/done"))
        .queue()
        .start_fetch()
        .fetched()
        .extracted()
        .processed();
    #[allow(path_statements)]
    {
        let committed = {
            use std::path::PathBuf;
            s.export_flushed(PathBuf::from("out/done.jsonl")).commit()
        };
        let _ = committed.queue();
        let _ = committed.start_fetch();
        let _ = committed.reopen_for_reexport();
    }
}
