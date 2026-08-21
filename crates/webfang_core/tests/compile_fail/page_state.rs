//! SC1 executable proof: illegal lifecycle transitions do NOT compile.
//!
//! Each case under `page_state/` attempts one illegal move (state skip,
//! non-recovery backward transition, COMMITTED escape, or self-transition).
//! trybuild asserts every case fails compilation — these moves have no
//! method in the [`Stateful`] typestate API, so failure is a type error,
//! never a runtime check.

#[test]
fn illegal_page_transitions_fail_to_compile() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/compile_fail/page_state/*.rs");
}
