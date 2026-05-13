//! Integration tests for [`Aliases::resolve`].

use aliases::{Aliases, ChatRef, UnknownAlias};
use std::collections::BTreeMap;

fn fixture() -> Aliases {
    let mut m = BTreeMap::new();
    m.insert("alerts".to_string(), -1_001_234_567_890_i64);
    m.insert("me".to_string(), 12_345_678_i64);
    Aliases::new(m)
}

#[test]
fn numeric_passes_through() {
    let a = fixture();
    let id = a.resolve(&ChatRef::Id(42)).unwrap();
    assert_eq!(id, 42);
}

#[test]
fn known_alias_resolves() {
    let a = fixture();
    let id = a.resolve(&ChatRef::Name("alerts".into())).unwrap();
    assert_eq!(id, -1_001_234_567_890);
}

#[test]
fn unknown_alias_errors() {
    let a = fixture();
    let err = a.resolve(&ChatRef::Name("nope".into())).unwrap_err();
    assert!(matches!(err, UnknownAlias { name } if name == "nope"));
}

#[test]
fn names_lists_aliases_sorted() {
    let a = fixture();
    let names: Vec<&str> = a.names().collect();
    assert_eq!(names, vec!["alerts", "me"]);
}
