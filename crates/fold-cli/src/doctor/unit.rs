use std::collections::BTreeMap;

use super::{Check, Host, systemd_available};
use crate::service::MEMORY_SETTINGS;

const NAME: &str = "service memory";

/// The memory lines that the `[Service]` section of `text` lacks, as `Name=value` in the order `fld setup` writes them. `OOMPolicy` must be `continue`; a memory limit counts with any value, since the user may have chosen another one. The last assignment of a name wins, and an empty one resets it, as in systemd.
pub(crate) fn missing_memory_lines(text: &str) -> Vec<String> {
    let mut values = BTreeMap::new();
    let mut in_service = false;
    for line in text.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_service = section == "Service";
        } else if in_service && let Some((name, value)) = line.split_once('=') {
            values.insert(name.trim(), value.trim());
        }
    }
    MEMORY_SETTINGS
        .iter()
        .filter(|(name, wanted)| match values.get(name) {
            Some(value) if *name == "OOMPolicy" => value != wanted,
            Some(value) => value.is_empty(),
            None => true,
        })
        .map(|(name, wanted)| format!("{name}={wanted}"))
        .collect()
}

pub(crate) fn unit_check(text: &str) -> Check {
    let missing = missing_memory_lines(text);
    if missing.is_empty() {
        return Check::ok(
            NAME,
            "the unit keeps the daemon running when the OOM killer ends one process, and caps its memory",
        );
    }
    Check::warn(
        NAME,
        format!(
            "the unit lacks {}; the OOM killer can end the whole daemon and every worker with one process, and nothing caps the unit's memory",
            missing.join(", ")
        ),
        "`fld setup` writes the unit again",
    )
}

pub(crate) fn service_unit_check(host: &Host) -> Check {
    if !systemd_available(host) {
        return Check::not_applicable(NAME);
    }
    match host.unit_path().map(std::fs::read_to_string) {
        Some(Ok(text)) => unit_check(&text),
        _ => Check::ok(NAME, "the fold service is not installed"),
    }
}
