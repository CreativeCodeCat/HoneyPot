//! The error and context types every command and event handler uses.

pub use super::framework::Context;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
