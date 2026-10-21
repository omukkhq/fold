pub mod agents;
mod columns;
pub mod counters;
pub mod error;
pub mod events;
pub mod messages;
pub mod projects;
pub mod reviews;
pub mod store;
pub mod tasks;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod workspaces;

pub use error::StoreError;
/// The connection the store reads take, so a service runs several of them on one read transaction without depending on sqlx.
pub use sqlx::SqliteConnection;
/// The pool [`Store::read`] and [`Store::read_registry`] hand out, so a caller can hold one without depending on sqlx.
pub use sqlx::SqlitePool;
pub use store::{Store, WriteTx};
