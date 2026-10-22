// A file in this directory is compiled and run only through its `mod` line here.
#[cfg(test)]
pub mod common;

mod batch;
mod browse;
mod checkout;
mod compare;
mod diff;
mod dir_handles;
mod exclude;
mod gc;
mod git_export;
mod git_import;
mod git_remote;
mod import;
mod index;
mod integrate;
mod materialise;
mod objects;
mod paths;
mod platform;
mod restore;
mod revision_store;
mod scan;
mod snapshot;
mod sync;
mod watch;
