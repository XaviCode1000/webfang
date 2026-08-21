//! DISCOVERED → FETCHED state skip: `fetched` is only defined on the
//! FETCHING source impl, so calling it on DISCOVERED must not compile.

use webfang_core::domain::page_state::{Stateful};

fn main() {
    let s = Stateful::new(String::from("https://example.com/skip"));
    let _ = s.fetched();
}
