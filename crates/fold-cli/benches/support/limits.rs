use std::fmt;

use fold_core::codes;

/// The environment variable that multiplies every limit.
pub const SCALE_VARIABLE: &str = "FOLD_BENCH_LIMIT_SCALE";

/// The five runtime budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    IdleMemory,
    FullStatScan,
    ChangeVisibility,
    WorkspaceCreation,
    Integration,
}

/// The budgets at scale 1.0 as `(target, name, unit, limit)`. They bound the cost of fold itself and are measured on the development computer named in `docs/benchmarks.md`.
pub const BUDGETS: [(Target, &str, &str, f64); 5] = [
    (Target::IdleMemory, "idle_memory", "bytes", 60_000_000.0),
    (Target::FullStatScan, "full_stat_scan", "s", 2.0),
    (Target::ChangeVisibility, "change_visibility", "s", 1.0),
    (Target::WorkspaceCreation, "workspace_creation", "s", 20.0),
    (Target::Integration, "integration", "s", 2.0),
];

impl Target {
    fn budget(self) -> (Target, &'static str, &'static str, f64) {
        BUDGETS
            .into_iter()
            .find(|(target, ..)| *target == self)
            .unwrap_or(BUDGETS[0])
    }

    pub fn name(self) -> &'static str {
        self.budget().1
    }

    pub fn unit(self) -> &'static str {
        self.budget().2
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ScaleError {
    value: String,
}

impl ScaleError {
    pub fn code(&self) -> &'static str {
        codes::INVALID_CONFIG
    }
}

impl fmt::Display for ScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{SCALE_VARIABLE} must be a positive decimal, such as 3 or 1.5; got {:?}",
            self.value
        )
    }
}

impl std::error::Error for ScaleError {}

/// The factor that multiplies every limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(f64);

impl Scale {
    pub const ONE: Scale = Scale(1.0);

    /// An unset variable is 1.0; anything but a finite number above zero is refused.
    pub fn parse(value: Option<&str>) -> Result<Self, ScaleError> {
        let Some(text) = value else {
            return Ok(Self::ONE);
        };
        match text.trim().parse::<f64>() {
            Ok(factor) if factor.is_finite() && factor > 0.0 => Ok(Self(factor)),
            _ => Err(ScaleError {
                value: text.to_owned(),
            }),
        }
    }

    pub fn factor(self) -> f64 {
        self.0
    }

    pub fn limit(self, target: Target) -> f64 {
        target.budget().3 * self.0
    }
}
