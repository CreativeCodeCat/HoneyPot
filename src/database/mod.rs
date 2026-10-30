//! Storage. `connection` opens the SQLite files and `migrations` updates old schemas;
//! `guild_config` is the per-server config the core reads, and `repositories` the
//! stores features keep their data in.

pub mod connection;
pub mod guild_config;
pub mod migrations;
pub mod repositories;
