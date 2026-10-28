//! Economy cog: per-server wallets and banks, with work, payments and admin adjustments.

pub mod store;

use chrono::Utc;
use poise::serenity_prelude as serenity;
use rand::Rng;

use super::{Context, Error};
use store::{Balance, DailyRate, EcoError, Pocket, DEFAULT_DAILY};

const ICON: &str = "₪";
const GREEN: u32 = 0x57f287;
const PURPLE: u32 = 0x9b59b6;
const RED: u32 = 0xed4245;

/// Time between /eco work uses
const WORK_COOLDOWN_SECS: i64 = 6 * 60 * 60;
/// Later jobs pay more: the wage is a random 1-120, times the job's position
const JOBS: &[&str] = &[
    "Cleaner",
    "Waiter",
    "Bus Driver",
    "Teacher",
    "Builder",
    "Programmer",
    "Mechanic",
    "Chef",
    "Developer",
    "Oil Rigger",
    "Space Invader",
];

/// Formats an amount with the currency icon and thousands separators, e.g. "₪ 1,234".
pub(crate) fn money(amount: i64) -> String {
    let digits = amount.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{ICON} {grouped}", if amount < 0 { "-" } else { "" })
}

fn guild_id(ctx: Context<'_>) -> u64 {
    ctx.guild_id().expect("economy commands are guild_only").get()
}

/// Sends text in an embed: red when it starts with ❌, green otherwise.
async fn reply(ctx: Context<'_>, text: impl Into<String>, ephemeral: bool) -> Result<(), Error> {
    let text = text.into();
    let colour = if text.starts_with('❌') { RED } else { GREEN };
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(ephemeral)).await?;
    Ok(())
}

/// Sends a public embed, the style every successful economy action replies in.
async fn reply_embed(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Explains why an economy action failed, privately.
async fn reply_error(ctx: Context<'_>, error: EcoError, target: Option<serenity::UserId>) -> Result<(), Error> {
    let text = match error {
        EcoError::NoAccount => match target {
            Some(user) if user != ctx.author().id => format!("<@{user}> hasn't joined the economy yet."),
            _ => "You haven't joined the economy yet. Use `/eco join` first.".to_string(),
        },
        EcoError::RecipientNoAccount => {
            format!("<@{}> hasn't joined the economy yet.", target.unwrap_or(ctx.author().id))
        }
        EcoError::NotEnough(available) => format!("There isn't enough money for that. Only {} is available.", money(available)),
        EcoError::TooLarge => format!("That would take the balance over the maximum of {}.", money(1_000_000_000_000)),
        EcoError::OnCooldown(until) => format!("You can only work once every 6 hours. Try again <t:{until}:R>."),
        EcoError::DailyClaimed => {
            format!("You've already claimed your daily today. The next one is ready <t:{}:R>.", next_daily_reset())
        }
    };
    reply(ctx, format!("❌ {text}"), true).await
}

/// Today's day number (days since the Unix epoch, UTC); dailies reset at midnight UTC.
fn today() -> i64 {
    Utc::now().timestamp().div_euclid(86400)
}

/// Unix time of the next daily reset.
fn next_daily_reset() -> i64 {
    (today() + 1) * 86400
}

fn check_amount(amount: i64) -> Option<&'static str> {
    (amount < 1).then_some("❌ The amount must be at least 1.")
}

/// Economy commands
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Economy",
    subcommands("join", "work", "daily", "balance", "deposit", "withdraw", "pay", "dailyrate", "wallet", "bank"),
    subcommand_required
)]
pub async fn eco(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Join the economy
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn join(ctx: Context<'_>) -> Result<(), Error> {
    let created = ctx.data().economy.lock().unwrap().join(guild_id(ctx), ctx.author().id.get())?;
    if created {
        let embed = serenity::CreateEmbed::new()
            .colour(GREEN)
            .description(format!("<@{}> has joined the economy with {} in the bank.", ctx.author().id, money(store::STARTING_BANK)));
        reply_embed(ctx, embed).await
    } else {
        reply(ctx, "❌ You've already joined the economy.", true).await
    }
}

/// Work a random job (once every 6 hours)
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn work(ctx: Context<'_>) -> Result<(), Error> {
    let (job, wage) = {
        let mut rng = rand::rng();
        let index = rng.random_range(0..JOBS.len());
        (JOBS[index], rng.random_range(1..=120) * (index as i64 + 1))
    };

    let now = Utc::now().timestamp();
    let result = ctx.data().economy.lock().unwrap().work(guild_id(ctx), ctx.author().id.get(), wage, now, WORK_COOLDOWN_SECS)?;
    match result {
        Ok(_) => {
            let embed = serenity::CreateEmbed::new()
                .colour(PURPLE)
                .description(format!("<@{}> You worked as a {job} and earned {}", ctx.author().id, money(wage)));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            Ok(())
        }
        Err(e) => reply_error(ctx, e, None).await,
    }
}

/// The daily payout range for a member: their highest role with a rate set,
/// otherwise the server default. Returns the rate and the role's name, if any.
fn daily_rate_for(ctx: Context<'_>, rates: &[DailyRate], member_roles: &[serenity::RoleId]) -> (DailyRate, Option<String>) {
    let default = rates.iter().find(|r| r.role_id == 0).copied().unwrap_or(DEFAULT_DAILY);
    let Some(guild) = ctx.guild() else { return (default, None) };
    member_roles
        .iter()
        .filter_map(|id| guild.roles.get(id))
        .filter_map(|role| rates.iter().find(|r| r.role_id == role.id.get()).map(|rate| (role, *rate)))
        .max_by_key(|(role, _)| role.position)
        .map(|(role, rate)| (rate, Some(role.name.clone())))
        .unwrap_or((default, None))
}

/// Claim your daily reward (once a day, resets at midnight UTC)
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn daily(ctx: Context<'_>) -> Result<(), Error> {
    let rates = ctx.data().economy.lock().unwrap().daily_rates(guild_id(ctx))?;
    let roles = ctx.author_member().await.map(|m| m.roles.clone()).unwrap_or_default();
    let (rate, role_name) = daily_rate_for(ctx, &rates, &roles);
    let amount = rand::rng().random_range(rate.min..=rate.max);

    let result = ctx.data().economy.lock().unwrap().claim_daily(guild_id(ctx), ctx.author().id.get(), amount, today())?;
    match result {
        Ok(balance) => {
            let mut text = format!("<@{}> claimed their daily reward of {}!", ctx.author().id, money(amount));
            if let Some(role) = role_name {
                text += &format!("\n*{role} reward: {} – {}*", money(rate.min), money(rate.max));
            }
            text += &format!("\nWallet: {} · Next daily <t:{}:R>", money(balance.wallet), next_daily_reset());
            let embed = serenity::CreateEmbed::new().colour(PURPLE).title("📅 Daily Reward").description(text);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            Ok(())
        }
        Err(e) => reply_error(ctx, e, None).await,
    }
}

/// Daily reward settings (Manage Server)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Economy",
    subcommands("dailyrate_set", "dailyrate_remove", "dailyrate_list"),
    subcommand_required
)]
async fn dailyrate(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set the daily reward range for a role, or the default for everyone
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "set",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn dailyrate_set(
    ctx: Context<'_>,
    #[description = "Smallest daily reward"]
    #[min = 1]
    #[max = 1_000_000_000]
    min: i64,
    #[description = "Largest daily reward"]
    #[min = 1]
    #[max = 1_000_000_000]
    max: i64,
    #[description = "Role this range is for (leave empty to set the default)"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    if min < 1 || max > 1_000_000_000 || min > max {
        return reply(ctx, "❌ The minimum must be at least 1 and no more than the maximum (up to 1,000,000,000).", true).await;
    }
    let role_id = role.as_ref().map(|r| r.id.get()).unwrap_or(0);
    ctx.data().economy.lock().unwrap().set_daily_rate(guild_id(ctx), DailyRate { role_id, min, max })?;

    let who = role.map(|r| format!("<@&{}>", r.id)).unwrap_or_else(|| "everyone (default)".to_string());
    reply(ctx, format!("✅ Daily reward for {who} is now {} – {}.", money(min), money(max)), true).await
}

/// Remove a role's daily reward range
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "remove",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn dailyrate_remove(
    ctx: Context<'_>,
    #[description = "Role to remove (leave empty to reset the default)"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    let role_id = role.as_ref().map(|r| r.id.get()).unwrap_or(0);
    let removed = ctx.data().economy.lock().unwrap().remove_daily_rate(guild_id(ctx), role_id)?;
    let text = match (removed, role) {
        (true, Some(role)) => format!("✅ Removed the daily reward for <@&{}>.", role.id),
        (true, None) => format!("✅ Reset the default daily reward to {} – {}.", money(DEFAULT_DAILY.min), money(DEFAULT_DAILY.max)),
        (false, Some(role)) => format!("❌ <@&{}> doesn't have a daily reward set.", role.id),
        (false, None) => "❌ The default daily reward is already the built-in one.".to_string(),
    };
    reply(ctx, text, true).await
}

/// Show the daily reward ranges
#[poise::command(slash_command, prefix_command, guild_only, rename = "list", category = "Economy")]
async fn dailyrate_list(ctx: Context<'_>) -> Result<(), Error> {
    let rates = ctx.data().economy.lock().unwrap().daily_rates(guild_id(ctx))?;
    let default = rates.iter().find(|r| r.role_id == 0).copied().unwrap_or(DEFAULT_DAILY);

    let mut lines = vec![format!("**Everyone:** {} – {}", money(default.min), money(default.max))];
    lines.extend(
        rates
            .iter()
            .filter(|r| r.role_id != 0)
            .map(|r| format!("<@&{}>: {} – {}", r.role_id, money(r.min), money(r.max))),
    );
    let embed = serenity::CreateEmbed::new()
        .colour(PURPLE)
        .title("📅 Daily Rewards")
        .description(lines.join("\n"))
        .footer(serenity::CreateEmbedFooter::new("Members get the range of their highest role that has one."));
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Check your balance, or another member's
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn balance(
    ctx: Context<'_>,
    #[description = "The user whose balance to check"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or(ctx.author());
    let balance = ctx.data().economy.lock().unwrap().balance(guild_id(ctx), user.id.get())?;
    let Some(Balance { wallet, bank }) = balance else {
        return reply_error(ctx, EcoError::NoAccount, Some(user.id)).await;
    };

    let embed = serenity::CreateEmbed::new()
        .title(user.tag())
        .colour(GREEN)
        .field("Wallet", format!("`{}`", money(wallet)), true)
        .field("Bank", format!("`{}`", money(bank)), true)
        .field("Total", format!("`{}`", money(wallet + bank)), true)
        .timestamp(serenity::Timestamp::now());
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Move money between your wallet and bank, and report the result.
async fn move_money(ctx: Context<'_>, amount: i64, from: Pocket) -> Result<(), Error> {
    if let Some(error) = check_amount(amount) {
        return reply(ctx, error, true).await;
    }
    let result = ctx.data().economy.lock().unwrap().transfer(guild_id(ctx), ctx.author().id.get(), amount, from)?;
    match result {
        Ok(balance) => {
            let action = if from == Pocket::Wallet { "deposited" } else { "withdrew" };
            let embed = serenity::CreateEmbed::new()
                .colour(GREEN)
                .description(format!("<@{}> {action} {}.", ctx.author().id, money(amount)))
                .field("Wallet", format!("`{}`", money(balance.wallet)), true)
                .field("Bank", format!("`{}`", money(balance.bank)), true);
            reply_embed(ctx, embed).await
        }
        Err(e) => reply_error(ctx, e, None).await,
    }
}

/// Deposit money from your wallet into your bank
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn deposit(
    ctx: Context<'_>,
    #[description = "The amount to deposit"]
    #[min = 1]
    amount: i64,
) -> Result<(), Error> {
    move_money(ctx, amount, Pocket::Wallet).await
}

/// Withdraw money from your bank into your wallet
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn withdraw(
    ctx: Context<'_>,
    #[description = "The amount to withdraw"]
    #[min = 1]
    amount: i64,
) -> Result<(), Error> {
    move_money(ctx, amount, Pocket::Bank).await
}

/// Pay another member from your wallet
#[poise::command(slash_command, prefix_command, guild_only, category = "Economy")]
async fn pay(
    ctx: Context<'_>,
    #[description = "The user to pay"] user: serenity::User,
    #[description = "The amount to pay"]
    #[min = 1]
    amount: i64,
) -> Result<(), Error> {
    if let Some(error) = check_amount(amount) {
        return reply(ctx, error, true).await;
    }
    if user.id == ctx.author().id {
        return reply(ctx, "❌ You can't pay yourself.", true).await;
    }
    if user.bot {
        return reply(ctx, "❌ Bots can't have money.", true).await;
    }

    let result = ctx.data().economy.lock().unwrap().pay(guild_id(ctx), ctx.author().id.get(), user.id.get(), amount)?;
    match result {
        Ok(_) => {
            let embed = serenity::CreateEmbed::new()
                .colour(GREEN)
                .description(format!("<@{}> has paid <@{}> {}.", ctx.author().id, user.id, money(amount)));
            reply_embed(ctx, embed).await
        }
        Err(e) => reply_error(ctx, e, Some(user.id)).await,
    }
}

/// Admin adjustment of a member's wallet or bank, and the reply.
async fn adjust(ctx: Context<'_>, user: Option<serenity::User>, amount: i64, pocket: Pocket, add: bool) -> Result<(), Error> {
    if let Some(error) = check_amount(amount) {
        return reply(ctx, error, true).await;
    }
    let user_id = user.map(|u| u.id).unwrap_or(ctx.author().id);
    let delta = if add { amount } else { -amount };

    let result = ctx.data().economy.lock().unwrap().adjust(guild_id(ctx), user_id.get(), pocket, delta)?;
    match result {
        Ok(balance) => {
            let (verb, preposition) = if add { ("added", "to") } else { ("removed", "from") };
            let pocket_name = if pocket == Pocket::Wallet { "wallet" } else { "bank" };
            let embed = serenity::CreateEmbed::new()
                .colour(PURPLE)
                .description(format!("<@{}> {verb} {} {preposition} <@{user_id}>'s {pocket_name}.", ctx.author().id, money(amount)))
                .field("Wallet", format!("`{}`", money(balance.wallet)), true)
                .field("Bank", format!("`{}`", money(balance.bank)), true);
            reply_embed(ctx, embed).await
        }
        Err(e) => reply_error(ctx, e, Some(user_id)).await,
    }
}

/// Wallet admin commands
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Economy",
    subcommands("wallet_add", "wallet_remove"),
    subcommand_required
)]
async fn wallet(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add money to a wallet (Manage Server)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "add",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn wallet_add(
    ctx: Context<'_>,
    #[description = "The amount to add"]
    #[min = 1]
    amount: i64,
    #[description = "The user to give money to (defaults to you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    adjust(ctx, user, amount, Pocket::Wallet, true).await
}

/// Remove money from a wallet (Manage Server)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "remove",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn wallet_remove(
    ctx: Context<'_>,
    #[description = "The amount to remove"]
    #[min = 1]
    amount: i64,
    #[description = "The user to take money from (defaults to you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    adjust(ctx, user, amount, Pocket::Wallet, false).await
}

/// Bank admin commands
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Economy",
    subcommands("bank_add", "bank_remove"),
    subcommand_required
)]
async fn bank(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add money to a bank (Manage Server)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "add",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn bank_add(
    ctx: Context<'_>,
    #[description = "The amount to add"]
    #[min = 1]
    amount: i64,
    #[description = "The user to give money to (defaults to you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    adjust(ctx, user, amount, Pocket::Bank, true).await
}

/// Remove money from a bank (Manage Server)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "remove",
    category = "Economy",
    required_permissions = "MANAGE_GUILD"
)]
async fn bank_remove(
    ctx: Context<'_>,
    #[description = "The amount to remove"]
    #[min = 1]
    amount: i64,
    #[description = "The user to take money from (defaults to you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    adjust(ctx, user, amount, Pocket::Bank, false).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_money() {
        assert_eq!(money(0), "₪ 0");
        assert_eq!(money(999), "₪ 999");
        assert_eq!(money(1234), "₪ 1,234");
        assert_eq!(money(1_000_000), "₪ 1,000,000");
        assert_eq!(money(-5000), "-₪ 5,000");
    }
}
