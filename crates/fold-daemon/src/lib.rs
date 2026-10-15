pub mod clock;
pub mod config;
pub mod daemon;
pub mod http;
pub mod jobs;
pub mod live;
pub mod mcp;
pub mod projects;
pub mod push;
pub mod services;
pub mod socket;

pub use clock::SystemClock;
pub use config::DaemonConfig;
pub use daemon::{Daemon, DaemonError, RunningDaemon, shutdown_signal};

/// Sets the failpoints that `FAILPOINTS` names, such as `integration_in_transaction=sleep(600000)`, for a `fld serve` built with the `failpoints` feature, so a crash test can stop the real process at a chosen point. They stay set until the scenario drops; only one scenario exists per process at a time.
#[cfg(feature = "failpoints")]
pub fn failpoints_from_env() -> fail::FailScenario<'static> {
    fail::FailScenario::setup()
}
