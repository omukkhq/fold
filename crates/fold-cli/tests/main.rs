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
#[path = "../benches/support/report.rs"]
pub mod report;
#[cfg(test)]
#[path = "../benches/support/targets.rs"]
pub mod targets;

mod adapter;
mod agents_section;
mod bench_support;
mod binary;
mod claude_files;
mod claude_start;
#[cfg(feature = "failpoints")]
mod crash_sigkill;
mod daemon_client;
mod daemon_lock;
#[cfg(not(feature = "dev-seed"))]
mod dev_command;
#[cfg(feature = "dev-seed")]
mod dev_stub;
mod doctor;
mod gc;
mod git;
mod git_identity;
mod hook_session_start;
mod hook_stop;
mod init;
mod init_folder;
mod news;
mod remote;
mod restart;
#[cfg(feature = "dev-seed")]
mod seed_dev;
mod service_install;
mod shell_env;
mod templates;
mod token;
mod with_git;
