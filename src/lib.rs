//! The HoneyPot Discord bot. `core` connects to Discord and routes commands and events to
//! `features`; `database` holds the shared storage and `shared` the common helpers.

pub mod core;
pub mod database;
pub mod features;
pub mod shared;

pub use shared::{log, pagination, render};
