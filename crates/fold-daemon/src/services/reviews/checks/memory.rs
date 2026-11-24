use std::sync::Arc;

/// The least free memory a check run waits for when the host sets no floor: a test suite with many processes easily takes a few gigabytes.
pub const MIN_FLOOR_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The share of the total memory, in percent, that the default floor keeps free on a large machine.
pub const FLOOR_PERCENT: u64 = 10;

/// The two values of `/proc/meminfo` the memory gate reads, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
}

/// `MemTotal` and `MemAvailable` of `/proc/meminfo`, whose values are in KiB. `None` when either line is missing or unreadable, as on a kernel older than 3.14.
pub fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let slot = match name.trim() {
            "MemTotal" => &mut total,
            "MemAvailable" => &mut available,
            _ => continue,
        };
        let kib: u64 = value.trim().trim_end_matches("kB").trim().parse().ok()?;
        *slot = Some(kib.saturating_mul(1024));
    }
    Some(MemInfo {
        total: total?,
        available: available?,
    })
}

/// The free memory a run waits for: the host's `floor`, else the larger of [`MIN_FLOOR_BYTES`] and [`FLOOR_PERCENT`] of the total.
pub fn memory_floor(total: u64, floor: Option<u64>) -> u64 {
    floor.unwrap_or_else(|| MIN_FLOOR_BYTES.max(total / 100 * FLOOR_PERCENT))
}

/// A run with its permit starts its commands only when the available memory is at or above the floor.
pub fn enough_memory(info: MemInfo, floor: Option<u64>) -> bool {
    info.available >= memory_floor(info.total, floor)
}

/// Where the memory gate reads the machine's memory. `None` from the source skips the gate, as on a system without `/proc/meminfo`.
pub type MemorySource = Arc<dyn Fn() -> Option<MemInfo> + Send + Sync>;

/// The machine's `/proc/meminfo` on Linux; no source elsewhere, so other systems skip the gate.
pub fn system_source() -> Option<MemorySource> {
    if cfg!(target_os = "linux") {
        Some(Arc::new(|| {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .as_deref()
                .and_then(parse_meminfo)
        }))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn meminfo_gives_total_and_available_in_bytes() {
        let text = "MemTotal:       32768000 kB\nMemFree:         1000 kB\nMemAvailable:    8192000 kB\nBuffers: 1 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some(MemInfo {
                total: 32_768_000 * 1024,
                available: 8_192_000 * 1024,
            })
        );
        assert_eq!(parse_meminfo("MemTotal: 10 kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: x kB\nMemAvailable: 1 kB\n"), None);
        assert_eq!(
            parse_meminfo("garbage\nMemAvailable: 1 kB\nMemTotal: 2 kB"),
            Some(MemInfo {
                total: 2048,
                available: 1024
            })
        );
    }

    #[test]
    fn the_default_floor_is_four_gib_or_a_tenth_of_the_total() {
        assert_eq!(memory_floor(16 * GIB, None), 4 * GIB);
        assert_eq!(memory_floor(100 * GIB, None), 10 * GIB);
        assert_eq!(memory_floor(100 * GIB, Some(GIB)), GIB);
    }

    #[test]
    fn a_run_starts_at_or_above_the_floor_and_waits_below_it() {
        let at = |available| MemInfo {
            total: 16 * GIB,
            available,
        };
        assert!(!enough_memory(at(4 * GIB - 1), None), "below");
        assert!(enough_memory(at(4 * GIB), None), "at");
        assert!(enough_memory(at(4 * GIB + 1), None), "above");
        assert!(enough_memory(at(GIB), Some(GIB)));
        assert!(!enough_memory(at(GIB), Some(2 * GIB)));
    }

    #[test]
    fn the_system_source_reads_this_machine_on_linux() {
        let source = system_source();
        if cfg!(target_os = "linux") {
            let info = source.and_then(|read| read()).unwrap();
            assert!(info.total > 0 && info.available <= info.total);
        } else {
            assert!(source.is_none());
        }
    }
}
