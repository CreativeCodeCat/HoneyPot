//! Reading a command's options: each option type says how Discord should ask for it and how to
//! turn what Discord sends back into the Rust value.

use ::serenity::all as serenity;

use super::{BoxFuture, Context};
use crate::core::error::Error;

/// A type a command can take as an option.
pub trait SlashArg: Sized {
    /// Sets the option's type, plus any limits or choices that come with it
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption;
    /// The value, or why it can't be read (finishing "The `name` option ...")
    fn extract<'a>(ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>>;
}

/// An enum used as an option with fixed choices; `#[derive(Choice)]` implements it.
pub trait Choice {
    /// The name the choice shows in Discord
    fn name(&self) -> &'static str;
}

/// The option `name`, which Discord requires.
pub async fn required_arg<T: SlashArg>(ctx: Context<'_>, name: &str) -> Result<T, Error> {
    optional_arg(ctx, name).await?.ok_or_else(|| format!("The `{name}` option is missing.").into())
}

/// The option `name`, if it was given.
pub async fn optional_arg<T: SlashArg>(ctx: Context<'_>, name: &str) -> Result<Option<T>, Error> {
    let Some(option) = ctx.args.iter().find(|o| o.name == name) else { return Ok(None) };
    T::extract(ctx, &option.value).await.map(Some).map_err(|e| format!("The `{name}` option {e}.").into())
}

/// What an autocomplete function can return.
pub trait IntoChoices {
    fn into_choices(self) -> Vec<serenity::AutocompleteChoice>;
}

impl IntoChoices for Vec<serenity::AutocompleteChoice> {
    fn into_choices(self) -> Vec<serenity::AutocompleteChoice> {
        self
    }
}

impl IntoChoices for Vec<String> {
    fn into_choices(self) -> Vec<serenity::AutocompleteChoice> {
        self.into_iter().map(|s| serenity::AutocompleteChoice::new(s.clone(), s)).collect()
    }
}

impl SlashArg for String {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::String)
    }

    fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            match value {
                serenity::ResolvedValue::String(s) => Ok(s.to_string()),
                _ => Err("isn't text".to_string()),
            }
        })
    }
}

impl SlashArg for bool {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::Boolean)
    }

    fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            match value {
                serenity::ResolvedValue::Boolean(b) => Ok(*b),
                _ => Err("isn't true or false".to_string()),
            }
        })
    }
}

/// Discord's integer options only go this far either way
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

macro_rules! integer_arg {
    ($($ty:ty),*) => {$(
        impl SlashArg for $ty {
            fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
                option
                    .kind(serenity::CommandOptionType::Integer)
                    .min_number_value((<$ty>::MIN as i128).max(-MAX_SAFE_INTEGER as i128) as f64)
                    .max_number_value((<$ty>::MAX as i128).min(MAX_SAFE_INTEGER as i128) as f64)
            }

            fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
                Box::pin(async move {
                    match value {
                        serenity::ResolvedValue::Integer(n) => <$ty>::try_from(*n).map_err(|_| "is out of range".to_string()),
                        _ => Err("isn't a whole number".to_string()),
                    }
                })
            }
        }
    )*};
}

integer_arg!(i64, i32, u64, u32, u16, u8, usize);

impl SlashArg for f64 {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::Number)
    }

    fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            match value {
                serenity::ResolvedValue::Number(n) => Ok(*n),
                serenity::ResolvedValue::Integer(n) => Ok(*n as f64),
                _ => Err("isn't a number".to_string()),
            }
        })
    }
}

impl SlashArg for serenity::User {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::User)
    }

    fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            match value {
                serenity::ResolvedValue::User(user, _) => Ok((*user).clone()),
                _ => Err("isn't a user".to_string()),
            }
        })
    }
}

impl SlashArg for serenity::Member {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::User)
    }

    fn extract<'a>(ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            let serenity::ResolvedValue::User(user, _) = value else { return Err("isn't a user".to_string()) };
            let guild_id = ctx.guild_id().ok_or("only works in a server")?;
            guild_id.member(ctx, user.id).await.map_err(|_| "isn't a member of this server".to_string())
        })
    }
}

impl SlashArg for serenity::GuildChannel {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::Channel)
    }

    fn extract<'a>(ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            let serenity::ResolvedValue::Channel(channel) = value else { return Err("isn't a channel".to_string()) };
            match channel.id.to_channel(ctx).await {
                Ok(serenity::Channel::Guild(channel)) => Ok(channel),
                Ok(_) => Err("isn't a server channel".to_string()),
                Err(_) => Err("is a channel I can't see".to_string()),
            }
        })
    }
}

impl SlashArg for serenity::Role {
    fn create(option: serenity::CreateCommandOption) -> serenity::CreateCommandOption {
        option.kind(serenity::CommandOptionType::Role)
    }

    fn extract<'a>(_ctx: Context<'a>, value: &'a serenity::ResolvedValue<'a>) -> BoxFuture<'a, Result<Self, String>> {
        Box::pin(async move {
            match value {
                serenity::ResolvedValue::Role(role) => Ok((*role).clone()),
                _ => Err("isn't a role".to_string()),
            }
        })
    }
}
