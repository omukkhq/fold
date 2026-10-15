use std::fmt::Write as _;

use serde::Serialize;

use super::limits::{Scale, Target};

/// One row of `results.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub target: String,
    pub value: f64,
    pub unit: String,
    pub limit: f64,
    pub scale: f64,
    pub pass: bool,
}

#[derive(Debug, Default)]
pub struct Report {
    entries: Vec<Entry>,
}

impl Report {
    /// A value above its scaled limit does not pass.
    pub fn record(&mut self, target: Target, value: f64, scale: Scale) {
        let limit = scale.limit(target);
        self.entries.push(Entry {
            target: target.name().to_owned(),
            value,
            unit: target.unit().to_owned(),
            limit,
            scale: scale.factor(),
            pass: value <= limit,
        });
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn all_pass(&self) -> bool {
        self.entries.iter().all(|entry| entry.pass)
    }

    /// The gate: non-zero when limits are enforced and any target missed its scaled limit.
    pub fn exit_code(&self, enforce: bool) -> u8 {
        u8::from(enforce && !self.all_pass())
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }

    pub fn table(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(
            text,
            "{:<20} {:>16} {:>6} {:>16} {:>6}  result",
            "target", "value", "unit", "limit", "scale"
        );
        for entry in &self.entries {
            let _ = writeln!(
                text,
                "{:<20} {:>16} {:>6} {:>16} {:>6}  {}",
                entry.target,
                number(entry.value),
                entry.unit,
                number(entry.limit),
                entry.scale,
                if entry.pass { "pass" } else { "FAIL" }
            );
        }
        text
    }

    /// The table with the host block, the date, the scale and the fold version.
    pub fn markdown(&self, host: &str, date: &str, version: &str, scale: Scale) -> String {
        let mut text = format!(
            "# Benchmark results\n\n- Date: {date}\n- fold version: {version}\n- Limit scale: {}\n\n## Host\n\n{host}\n\n## Results\n\n",
            scale.factor()
        );
        text.push_str(
            "| Target | Value | Unit | Limit | Scale | Pass |\n|---|---|---|---|---|---|\n",
        );
        for entry in &self.entries {
            let _ = writeln!(
                text,
                "| {} | {} | {} | {} | {} | {} |",
                entry.target,
                number(entry.value),
                entry.unit,
                number(entry.limit),
                entry.scale,
                if entry.pass { "yes" } else { "no" }
            );
        }
        text
    }
}

fn number(value: f64) -> String {
    if value >= 1000.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

/// The UTC calendar date of a Unix time in seconds, as `YYYY-MM-DD`.
pub fn utc_date(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
