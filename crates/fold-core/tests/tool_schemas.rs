use std::collections::BTreeMap;

use fold_core::tool_schemas::changed_tools;
use proptest::prelude::*;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(tool, hash)| ((*tool).to_owned(), (*hash).to_owned()))
        .collect()
}

#[test]
fn equal_maps_change_nothing() {
    let tools = map(&[("a", "1"), ("b", "2")]);
    assert!(changed_tools(&tools, &tools).is_empty());
    assert!(changed_tools(&BTreeMap::new(), &BTreeMap::new()).is_empty());
}

#[test]
fn an_added_tool_is_named() {
    let old = map(&[("a", "1")]);
    let new = map(&[("a", "1"), ("b", "2")]);
    assert_eq!(changed_tools(&old, &new), ["b"]);
}

#[test]
fn a_removed_tool_is_named() {
    let old = map(&[("a", "1"), ("b", "2")]);
    let new = map(&[("a", "1")]);
    assert_eq!(changed_tools(&old, &new), ["b"]);
}

#[test]
fn a_tool_with_another_hash_is_named() {
    let old = map(&[("a", "1"), ("b", "2")]);
    let new = map(&[("a", "1"), ("b", "3")]);
    assert_eq!(changed_tools(&old, &new), ["b"]);
}

#[test]
fn added_removed_and_changed_tools_come_sorted() {
    let old = map(&[("keep", "1"), ("gone", "1"), ("edit", "1")]);
    let new = map(&[("keep", "1"), ("edit", "2"), ("added", "1")]);
    assert_eq!(changed_tools(&old, &new), ["added", "edit", "gone"]);
}

#[test]
fn an_unknown_old_set_names_every_new_tool() {
    let new = map(&[("b", "2"), ("a", "1")]);
    assert_eq!(changed_tools(&BTreeMap::new(), &new), ["a", "b"]);
}

fn tools() -> impl Strategy<Value = BTreeMap<String, String>> {
    prop::collection::btree_map("[a-e]{1,2}", "[0-2]", 0..8)
}

proptest! {
    #[test]
    fn the_result_is_sorted_and_unique(old in tools(), new in tools()) {
        let changed = changed_tools(&old, &new);
        let mut expected = changed.clone();
        expected.sort();
        expected.dedup();
        prop_assert_eq!(changed, expected);
    }

    #[test]
    fn equal_maps_give_an_empty_result(tools in tools()) {
        prop_assert!(changed_tools(&tools, &tools).is_empty());
    }

    #[test]
    fn a_tool_is_named_exactly_when_its_hash_differs(old in tools(), new in tools()) {
        let changed = changed_tools(&old, &new);
        let names: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        for tool in names {
            prop_assert_eq!(changed.contains(tool), old.get(tool) != new.get(tool));
        }
    }

    #[test]
    fn the_diff_is_symmetric(old in tools(), new in tools()) {
        prop_assert_eq!(changed_tools(&old, &new), changed_tools(&new, &old));
    }
}
