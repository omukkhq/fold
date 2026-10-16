use std::fmt::Write as _;

use serde::Serialize;

use super::targets::Target;

/// One row of `results.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub target: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Default)]
pub struct Report {
    entries: Vec<Entry>,
}

impl Report {
    pub fn record(&mut self, target: Target, value: f64) {
        self.entries.push(Entry {
            target: target.name().to_owned(),
            value,
            unit: target.unit().to_owned(),
        });
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }

    pub fn table(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "{:<20} {:>16} {:>6}", "target", "value", "unit");
        for entry in &self.entries {
            let _ = writeln!(
                text,
                "{:<20} {:>16} {:>6}",
                entry.target,
                number(entry.value),
                entry.unit
            );
        }
        text
    }

    /// The table with the host block, the date and the fold version.
    pub fn markdown(&self, host: &str, date: &str, version: &str) -> String {
        let mut text = format!(
            "# Benchmark results\n\n- Date: {date}\n- fold version: {version}\n\n## Host\n\n{host}\n\n## Results\n\n"
        );
        text.push_str("| Target | Value | Unit |\n|---|---|---|\n");
        for entry in &self.entries {
            let _ = writeln!(
                text,
                "| {} | {} | {} |",
                entry.target,
                number(entry.value),
                entry.unit
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
