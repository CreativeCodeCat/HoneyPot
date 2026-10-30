//! The bot's slash command framework. Commands are written as plain async functions with
//! `#[command(...)]` (from the `honeypot-macros` crate), which turns each into a `fn() -> Command`.
//! The `Framework` registers them with Discord, runs the checks, parses their options and
//! hands every gateway event on to the event router.

mod args;
mod command;
mod context;
mod dispatch;
mod modal;
mod reply;

use std::future::Future;
use std::pin::Pin;

pub use args::{optional_arg, required_arg, Choice, IntoChoices, SlashArg};
pub use command::{AutocompleteFn, CheckFn, Command, ContextMenuAction, Parameter, SlashAction};
pub use context::Context;
pub use dispatch::{Framework, FrameworkError};
pub use honeypot_macros::{command, Choice};
pub use modal::{create_modal, execute_modal, find_modal_text, Modal, TextField};
pub use reply::{CreateReply, ReplyHandle};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
