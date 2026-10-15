// A file in this directory is compiled and run only through its `mod` line here.
#[cfg(test)]
pub mod common;
#[cfg(test)]
#[path = "../benches/support/fixture.rs"]
pub mod fixture;
#[cfg(test)]
#[path = "../../fold-daemon/tests/common/invariants.rs"]
pub mod invariants;
#[cfg(test)]
#[path = "../benches/support/limits.rs"]
pub mod limits;
#[cfg(test)]
#[path = "../benches/support/report.rs"]
pub mod report;

mod adapter;
mod agent_tag;
mod agents_section;
mod bench_support;
mod binary;
mod claude_files;
#[cfg(feature = "failpoints")]
mod crash_sigkill;
mod daemon_client;
#[cfg(not(feature = "dev-seed"))]
mod dev_command;
mod doctor;
mod gc;
mod git;
mod hook_session_start;
mod hook_stop;
mod init;
mod init_from;
mod remote;
#[cfg(feature = "dev-seed")]
mod seed_dev;
mod service_install;
mod templates;
mod token;
