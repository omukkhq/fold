use std::collections::BTreeMap;

/// The names of the tools whose schema differs between two sets of per-tool hashes: tools only `new` has, tools only `old` has, and tools in both with another hash. The names are sorted and unique because both inputs are keyed maps.
pub fn changed_tools(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    for (tool, hash) in new {
        if old.get(tool) != Some(hash) {
            changed.push(tool.clone());
        }
    }
    for tool in old.keys() {
        if !new.contains_key(tool) {
            changed.push(tool.clone());
        }
    }
    changed.sort();
    changed
}
