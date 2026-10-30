//! The bot's core: connecting to Discord, routing commands and events to features, and
//! deciding who may run what. Features never depend on each other through here.

pub mod command_router;
pub mod event_router;
pub mod gateway;
pub mod permissions;

pub mod error;
pub mod state;
