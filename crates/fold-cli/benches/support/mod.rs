pub mod daemon;
pub mod events;
pub mod fixture;
pub mod host;
pub mod integration;
pub mod report;
pub mod session;
pub mod targets;

pub type BenchResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn fail<T>(message: impl Into<String>) -> BenchResult<T> {
    Err(message.into().into())
}
