//! Dashboard moderation: finding members, seeing their history, and acting on them (kick,
//! ban, unban, timeout, quarantine) the way the moderation commands do. Only the server's
//! owner and admins (and the bot's owners) can act; people with only Manage Server can look.

use std::collections::HashMap;

use ::serenity::all as serenity;
use axum::extract::{Path, Query, State as AxumState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::dashboard::{json_error, Dashboard};
use crate::features::moderation::{quarantine, suspicious_accounts, MAX_TIMEOUT_SECS};
use crate::log;

/// Members a search returns at most
const SEARCH_SIZE: usize = 50;
/// Longest reason kept, as Discord's audit log allows
const MAX_REASON: usize = 400;

fn member_json(guild: &serenity::Guild, m: &serenity::Member) -> Value {
    let top = guild.member_highest_role(m);
    let timed_out = m.communication_disabled_until.filter(|t| *t > serenity::Timestamp::now()).map(|t| t.unix_timestamp());
    json!({
        "id": m.user.id.get().to_string(),
        "name": m.display_name(),
        "username": m.user.name,
        "avatar": m.face(),
        "bot": m.user.bot,
        "joined": m.joined_at.map(|t| t.unix_timestamp()),
        "created": m.user.id.created_at().unix_timestamp(),
        "top_role": top.map(|r| json!({ "name": r.name, "colour": r.colour.0 })),
        "timed_out_until": timed_out,
    })
}

/// Finds members by name or ID (`?q=`), or the newest to join if no search is given.
pub(super) async fn members(
    AxumState(dash): AxumState<Dashboard>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (guild_id, can_act) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let Some(guild) = dash.ctx.cache.guild(guild_id) else { return json_error(StatusCode::NOT_FOUND, "The bot isn't in that server") };
    let q = query.get("q").map(|q| q.trim().to_lowercase()).unwrap_or_default();
    let mut found: Vec<&serenity::Member> = guild
        .members
        .values()
        .filter(|m| {
            q.is_empty()
                || m.user.id.get().to_string() == q
                || m.user.name.to_lowercase().contains(&q)
                || m.display_name().to_lowercase().contains(&q)
        })
        .collect();
    let total = found.len();
    // Closest matches first: names that start with the search, then the newest members
    found.sort_by_key(|m| (!m.display_name().to_lowercase().starts_with(&q), std::cmp::Reverse(m.joined_at.map(|t| t.unix_timestamp()))));
    let list: Vec<Value> = found.into_iter().take(SEARCH_SIZE).map(|m| member_json(&guild, m)).collect();
    let quarantined = dash.data.quarantine.lock().unwrap().list(guild_id).map(|l| l.len()).unwrap_or(0);
    Json(json!({ "members": list, "total": total, "member_count": guild.member_count, "can_act": can_act, "quarantined": quarantined })).into_response()
}

/// One member, or a user who isn't one (like someone banned): who they are, their roles,
/// their history with the bot, and what can be done to them.
pub(super) async fn member(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path((id, user)): Path<(String, String)>) -> Response {
    let (guild_id, can_act) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let Some(user_id) = user.parse::<u64>().ok().filter(|&u| u != 0) else { return json_error(StatusCode::BAD_REQUEST, "That isn't a user ID") };
    let gid = serenity::GuildId::new(guild_id);
    let uid = serenity::UserId::new(user_id);

    let cached = dash.ctx.cache.guild(guild_id).and_then(|g| g.members.get(&uid).map(|m| (member_json(&g, m), roles_json(&g, m))));
    let (profile, roles, is_member) = match cached {
        Some((profile, roles)) => (profile, roles, true),
        None => match uid.to_user(&dash.ctx).await {
            Ok(u) => (
                json!({
                    "id": user_id.to_string(), "name": u.display_name(), "username": u.name, "avatar": u.face(), "bot": u.bot,
                    "joined": null, "created": uid.created_at().unix_timestamp(), "top_role": null, "timed_out_until": null,
                }),
                Vec::new(),
                false,
            ),
            Err(_) => return json_error(StatusCode::NOT_FOUND, "There's no user with that ID"),
        },
    };
    let ban = if is_member { None } else { dash.ctx.http.get_ban(gid, uid).await.ok().flatten() };

    let data = &dash.data;
    let incidents: Vec<Value> = data
        .incidents
        .lock()
        .unwrap()
        .list(guild_id, Some(user_id))
        .unwrap_or_default()
        .into_iter()
        .map(|i| json!({ "id": i.id, "action": i.action, "reason": i.reason, "moderator_id": i.moderator_id.to_string(), "at": i.created_at }))
        .collect();
    let quarantined = data.quarantine.lock().unwrap().get(guild_id, user_id).ok().flatten().map(|q| {
        json!({ "reason": q.reason, "source": q.source, "at": q.quarantined_at, "roles": q.saved_roles.iter().map(u64::to_string).collect::<Vec<_>>() })
    });
    let staff_roles = data.staff_policy.lock().unwrap().get(guild_id, user_id).ok().flatten();
    let level = data.levels.lock().unwrap().member(guild_id, user_id).ok().flatten();
    let balance = data.economy.lock().unwrap().balance(guild_id, user_id).ok().flatten();
    let afk = data.afk.lock().unwrap().get(guild_id, user_id).ok().flatten();
    let score = if data.is_cog_enabled(guild_id, suspicious_accounts::COG_NAME) {
        suspicious_accounts::score_for(&dash.ctx, data, gid, uid).await.map(|s| {
            json!({ "total": s.total, "reasons": s.reasons.iter().map(|r| json!({ "points": r.points, "text": r.text })).collect::<Vec<_>>() })
        })
    } else {
        None
    };
    let quarantine_ready = data.is_cog_enabled(guild_id, quarantine::COG_NAME)
        && data.quarantine.lock().unwrap().settings(guild_id).is_ok_and(|s| s.role_id.is_some());

    let mut history = json!({ "incidents": incidents });
    let names = dash.names(guild_id, &history);
    history["names"] = Value::Object(names);
    Json(json!({
        "user": profile,
        "member": is_member,
        "roles": roles,
        "banned": ban.map(|b| json!({ "reason": b.reason })),
        "quarantined": quarantined,
        "staff_policy_roles": staff_roles.map(|r| r.iter().map(u64::to_string).collect::<Vec<_>>()),
        "level": level.map(|l| json!({ "xp": l.xp, "rep": l.rep, "position": l.position })),
        "balance": balance.map(|b| json!({ "wallet": b.wallet, "bank": b.bank })),
        "afk": afk.map(|(reason, since)| json!({ "reason": reason, "since": since })),
        "score": score,
        "history": history,
        "can_act": can_act,
        "quarantine_ready": quarantine_ready,
        "max_timeout_secs": MAX_TIMEOUT_SECS,
    }))
    .into_response()
}

fn roles_json(guild: &serenity::Guild, m: &serenity::Member) -> Vec<Value> {
    let mut roles: Vec<&serenity::Role> = m.roles.iter().filter_map(|r| guild.roles.get(r)).collect();
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
    roles.into_iter().map(|r| json!({ "id": r.id.get().to_string(), "name": r.name, "colour": r.colour.0 })).collect()
}

/// Why the person signed in (or the bot) can't act on the member, if they can't: as the
/// commands check, themselves, the bot, the owner, or someone not below them.
fn hierarchy_problem(dash: &Dashboard, guild_id: u64, actor: u64, owner: bool, target: u64) -> Option<&'static str> {
    let bot = dash.ctx.cache.current_user().id.get();
    if target == actor {
        return Some("You can't moderate yourself");
    }
    if target == bot {
        return Some("The bot can't moderate itself");
    }
    let guild = dash.ctx.cache.guild(guild_id)?;
    if target == guild.owner_id.get() {
        return Some("The server owner can't be moderated");
    }
    // Not a member: nothing to compare, as only bans and unbans work on them
    let target_member = guild.members.get(&serenity::UserId::new(target))?;
    let top = |m: &serenity::Member| guild.member_highest_role(m).map_or(0, |r| r.position);
    // The bot's owners may not be in the server; then only the bot's own place counts
    if actor != guild.owner_id.get() {
        match guild.members.get(&serenity::UserId::new(actor)) {
            Some(me) if top(target_member) >= top(me) => return Some("That member's highest role is the same as or above yours"),
            None if !owner => return Some("You're not in this server any more"),
            _ => {}
        }
    }
    let bot_member = guild.members.get(&serenity::UserId::new(bot))?;
    if top(target_member) >= top(bot_member) {
        return Some("That member's highest role is the same as or above the bot's, so it can't moderate them");
    }
    None
}

/// Acts on a member: `kick`, `ban`, `unban`, `timeout`, `untimeout`, `quarantine` or `release`.
/// Takes `{"reason": "...", "seconds": 3600, "delete_days": 0}`; only timeout needs `seconds`.
pub(super) async fn act(
    AxumState(dash): AxumState<Dashboard>,
    headers: HeaderMap,
    Path((id, user, action)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let (guild_id, can_act) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    if !can_act {
        return json_error(StatusCode::FORBIDDEN, "Only the server's owner and admins can moderate from the dashboard");
    }
    let Some((actor, actor_name, owner)) = dash.actor(&headers) else {
        return super::dashboard::signed_out();
    };
    let Some(user_id) = user.parse::<u64>().ok().filter(|&u| u != 0) else { return json_error(StatusCode::BAD_REQUEST, "That isn't a user ID") };
    if action != "unban" && action != "release" {
        if let Some(problem) = hierarchy_problem(&dash, guild_id, actor, owner, user_id) {
            return json_error(StatusCode::FORBIDDEN, problem);
        }
    }
    let reason = body["reason"].as_str().map(str::trim).filter(|r| !r.is_empty());
    let audit = crate::features::truncate(&format!("{actor_name} (dashboard): {}", reason.unwrap_or("No reason given")), MAX_REASON);
    let gid = serenity::GuildId::new(guild_id);
    let uid = serenity::UserId::new(user_id);
    let ctx = &dash.ctx;
    let is_member = ctx.cache.guild(guild_id).is_some_and(|g| g.members.contains_key(&uid));
    let name = ctx
        .cache
        .guild(guild_id)
        .and_then(|g| g.members.get(&uid).map(|m| m.user.name.clone()))
        .or_else(|| ctx.cache.user(uid).map(|u| u.name.clone()))
        .unwrap_or_else(|| user_id.to_string());

    let result: Result<String, String> = match action.as_str() {
        "kick" | "ban" | "timeout" | "untimeout" | "quarantine" if !is_member && action != "ban" => Err("They aren't in the server".to_string()),
        "kick" => {
            notify(&dash, gid, uid, "kicked", reason).await;
            gid.kick_with_reason(ctx, uid, &audit).await.map(|_| format!("Kicked {name}")).map_err(discord)
        }
        "ban" => {
            let days = body["delete_days"].as_u64().unwrap_or(0).min(7) as u8;
            if is_member {
                notify(&dash, gid, uid, "banned", reason).await;
            }
            gid.ban_with_reason(ctx, uid, days, &audit).await.map(|_| format!("Banned {name}")).map_err(discord)
        }
        "unban" => match ctx.http.get_ban(gid, uid).await {
            Ok(Some(_)) => ctx.http.remove_ban(gid, uid, Some(&audit)).await.map(|_| format!("Unbanned {name}")).map_err(discord),
            Ok(None) => Err("They aren't banned".to_string()),
            Err(e) => Err(discord(e)),
        },
        "timeout" => {
            let seconds = body["seconds"].as_i64().unwrap_or(0);
            if !(60..=MAX_TIMEOUT_SECS).contains(&seconds) {
                Err("Pick a length between a minute and 28 days".to_string())
            } else {
                let until = serenity::Timestamp::now().unix_timestamp() + seconds;
                match serenity::Timestamp::from_unix_timestamp(until) {
                    Ok(at) => {
                        let edit = serenity::EditMember::new().disable_communication_until_datetime(at).audit_log_reason(&audit);
                        match gid.edit_member(ctx, uid, edit).await {
                            Ok(_) => {
                                notify(&dash, gid, uid, &format!("timed out until <t:{until}:F>"), reason).await;
                                Ok(format!("Timed out {name} for {}", length(seconds)))
                            }
                            Err(e) => Err(discord(e)),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
        }
        "untimeout" => {
            let edit = serenity::EditMember::new().enable_communication().audit_log_reason(&audit);
            gid.edit_member(ctx, uid, edit).await.map(|_| format!("Ended {name}'s timeout")).map_err(discord)
        }
        "quarantine" => match quarantine::isolate(ctx, &dash.data, gid, uid, reason.unwrap_or("No reason given"), &format!("{actor_name} (dashboard)")).await {
            Ok(true) => Ok(format!("Quarantined {name}")),
            Ok(false) => Err("Quarantine isn't set up here, or they're already quarantined".to_string()),
            Err(e) => Err(e.to_string()),
        },
        "release" => match quarantine::release(ctx, &dash.data, gid, uid, &format!("{actor_name} (dashboard)")).await {
            Ok(true) => Ok(format!("Released {name} from quarantine")),
            Ok(false) => Err("They aren't quarantined".to_string()),
            Err(e) => Err(e.to_string()),
        },
        _ => return json_error(StatusCode::NOT_FOUND, "There's no action by that name"),
    };
    match result {
        Ok(done) => {
            let entry = match reason {
                Some(reason) => format!("{done}: {reason}"),
                None => done.clone(),
            };
            dash.record(&headers, guild_id, entry);
            Json(json!({ "done": done })).into_response()
        }
        Err(e) => {
            log::warning(format!("Dashboard {action} on {user_id} in guild {guild_id} failed: {e}"));
            json_error(StatusCode::BAD_REQUEST, &e)
        }
    }
}

/// "2 days 3 hours", "45 minutes".
fn length(seconds: i64) -> String {
    let parts: Vec<String> = [(86400, "day"), (3600, "hour"), (60, "minute")]
        .iter()
        .scan(seconds, |left, &(size, name)| {
            let n = *left / size;
            *left %= size;
            Some((n > 0).then(|| format!("{n} {name}{}", if n == 1 { "" } else { "s" })))
        })
        .flatten()
        .collect();
    if parts.is_empty() { format!("{seconds} seconds") } else { parts.join(" ") }
}

/// Discord's reason, in words people can act on.
fn discord(e: serenity::Error) -> String {
    match &e {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) if r.status_code.as_u16() == 403 => {
            "The bot doesn't have permission to do that here".to_string()
        }
        _ => e.to_string(),
    }
}

/// Tells the member what happened, as the commands do. They may have DMs closed, which is fine.
async fn notify(dash: &Dashboard, guild_id: serenity::GuildId, user_id: serenity::UserId, what: &str, reason: Option<&str>) {
    let guild_name = dash.ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let text = format!("You have been {what} in **{guild_name}**.\n**Reason:** {}", reason.unwrap_or("No reason given"));
    if let Ok(channel) = user_id.create_dm_channel(&dash.ctx).await {
        let _ = channel.send_message(&dash.ctx, serenity::CreateMessage::new().content(text)).await;
    }
}

/// Bans listed at most, fetched 250 at a time
const MAX_BANS: usize = 1000;

/// The server's bans.
pub(super) async fn bans(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (guild_id, can_act) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let gid = serenity::GuildId::new(guild_id);
    let mut all = Vec::new();
    loop {
        let after = all.last().map(|b: &serenity::Ban| serenity::UserPagination::After(b.user.id));
        match dash.ctx.http.get_bans(gid, after, Some(250)).await {
            Ok(page) => {
                let done = page.len() < 250;
                all.extend(page);
                if done || all.len() >= MAX_BANS {
                    break;
                }
            }
            Err(e) => return json_error(StatusCode::BAD_GATEWAY, &discord(e)),
        }
    }
    let list: Vec<Value> = all
        .into_iter()
        .map(|b| json!({ "id": b.user.id.get().to_string(), "name": b.user.display_name(), "username": b.user.name, "avatar": b.user.face(), "reason": b.reason }))
        .collect();
    Json(json!({ "bans": list, "can_act": can_act })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_naturally() {
        assert_eq!(length(60), "1 minute");
        assert_eq!(length(2 * 86400 + 3 * 3600), "2 days 3 hours");
        assert_eq!(length(30), "30 seconds");
    }
}
