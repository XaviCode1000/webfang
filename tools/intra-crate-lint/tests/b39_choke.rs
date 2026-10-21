//! B39 — port-choke constructs.
//!
//! A method call named `dns_resolver` or `redirect` (any receiver) and any path
//! that mentions the concrete `ValidatingResolver` are reported as choke
//! constructs. Inside a test region they are not.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

const FIXTURE: &str = r#"fn a() {
    let _ = ValidatingResolver::new();
}
fn b() {
    let _ = ValidatingResolver::with_policy(policy);
}
fn c(_: ValidatingResolver) {}
fn d() {
    client.dns_resolver(resolver);
    client.redirect(policy);
}
#[cfg(test)]
mod tests {
    fn e() {
        let _ = ValidatingResolver::new();
        client.dns_resolver(resolver);
        client.redirect(policy);
    }
}
"#;

#[test]
fn choke_constructs_are_reported_and_test_constructs_are_not() {
    let tree = TempTree::new("b39");
    tree.write("b39.rs", FIXTURE);

    assert_eq!(
        tree.records(),
        vec![
            "2\tchoke\tValidatingResolver::new",
            "5\tchoke\tValidatingResolver::with_policy",
            "7\tchoke\tValidatingResolver",
            "9\tchoke\tdns_resolver",
            "10\tchoke\tredirect",
        ]
    );
}

#[test]
fn a_renamed_constructor_is_reported_with_its_segment() {
    let tree = TempTree::new("b39-allow-loopback");
    tree.write(
        "loopback.rs",
        "fn a() {\n\
         \x20   let _ = ValidatingResolver::with_allow_loopback(true);\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec!["2\tchoke\tValidatingResolver::with_allow_loopback"]
    );
}

#[test]
fn a_method_call_on_any_receiver_is_a_choke_construct() {
    let tree = TempTree::new("b39-receiver");
    tree.write(
        "receiver.rs",
        "fn a() {\n\
         \x20   anything.redirect(policy);\n\
         \x20   other.dns_resolver(resolver);\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec!["2\tchoke\tredirect", "3\tchoke\tdns_resolver"]
    );
}
