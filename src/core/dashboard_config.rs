//! Cog settings the dashboard can change: for each cog that has them, its fields (what kind
//! of value each takes, and its limits), how to read them and how to save them. The limits
//! match the slash commands' own, so the dashboard can't set anything they couldn't.

use ::serenity::all as serenity;
use serde_json::{json, Map, Value};

use crate::database::repositories::raid_protection::JoinerAction;
use crate::features::{leveling, Data};

/// What a field holds, and so how the page draws it and how it's checked.
pub enum Kind {
    /// A text or announcement channel
    Channel,
    /// Several text or announcement channels
    Channels,
    /// A channel category
    Category,
    Role,
    Roles,
    Number { min: i64, max: i64, unit: &'static str },
    Bool,
    /// One of these keys, shown by their labels
    Choice(&'static [(&'static str, &'static str)]),
}

pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// Whether it can be left unset (only for channels, categories and roles)
    pub optional: bool,
}

const fn field(key: &'static str, label: &'static str, help: &'static str, kind: Kind) -> Field {
    Field { key, label, help, kind, optional: false }
}

const fn optional(key: &'static str, label: &'static str, help: &'static str, kind: Kind) -> Field {
    Field { key, label, help, kind, optional: true }
}

type Read = fn(&Data, u64) -> Result<Map<String, Value>, String>;
/// Saves the checked values; every field is there. Returns an error to show if it can't.
type Write = fn(&Data, u64, &Map<String, Value>) -> Result<(), String>;

pub struct Form {
    /// The cog's name, as `COGS` has it
    pub cog: &'static str,
    /// What these settings do, shown above them
    pub about: &'static str,
    pub fields: Vec<Field>,
    pub read: Read,
    pub write: Write,
}

const JOINER_ACTIONS: &[(&str, &str)] =
    &[("none", "Nothing"), ("timeout", "Time them out"), ("kick", "Kick them"), ("ban", "Ban them"), ("quarantine", "Quarantine them")];

fn db(e: rusqlite::Error) -> String {
    crate::log::error(format!("Dashboard settings database error: {e}"));
    "The settings couldn't be read or saved".to_string()
}

fn id(v: Option<u64>) -> Value {
    v.map_or(Value::Null, |id| Value::String(id.to_string()))
}

fn ids(v: &[u64]) -> Value {
    Value::Array(v.iter().map(|id| Value::String(id.to_string())).collect())
}

// Readers for checked values: `check` has already made sure each is the right shape
fn get_id(values: &Map<String, Value>, key: &str) -> Option<u64> {
    values.get(key)?.as_str()?.parse().ok()
}

fn get_ids(values: &Map<String, Value>, key: &str) -> Vec<u64> {
    values.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str()?.parse().ok()).collect()
}

fn get_num(values: &Map<String, Value>, key: &str) -> i64 {
    values.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn get_bool(values: &Map<String, Value>, key: &str) -> bool {
    values.get(key).and_then(Value::as_bool).unwrap_or_default()
}

fn get_str<'a>(values: &'a Map<String, Value>, key: &str) -> &'a str {
    values.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Every cog the dashboard can change settings for.
pub fn forms() -> Vec<Form> {
    use leveling::store::{COOLDOWN_SECS_RANGE as CD, RATE_RANGE as RATE, WINDOW_MINS_RANGE as WIN, XP_RANGE as XP};
    vec![
        Form {
            cog: "Levels",
            about: "How members earn XP, like /rank-setup.",
            fields: vec![
                field("rate", "XP per level", "Extra XP each level needs over the last", Kind::Number { min: RATE.0, max: RATE.1, unit: "XP" }),
                field("base_xp", "XP per message", "For an ordinary message", Kind::Number { min: XP.0, max: XP.1, unit: "XP" }),
                field("max_xp", "XP for a reply", "For replying to someone else within the reply window; at least the XP per message", Kind::Number { min: XP.0, max: XP.1, unit: "XP" }),
                field("engagement_window_mins", "Reply window", "How soon after another member's message a reply earns the reply XP", Kind::Number { min: WIN.0, max: WIN.1, unit: "minutes" }),
                field("xp_cooldown_secs", "XP cooldown", "A member earns XP at most once in this long", Kind::Number { min: CD.0, max: CD.1, unit: "seconds" }),
                optional("levelup_channel_id", "Level-up channel", "Where level-ups are announced; the system channel if not set", Kind::Channel),
            ],
            read: |data, guild| {
                let s = data.levels.lock().unwrap().settings(guild).map_err(db)?;
                Ok(json!({
                    "rate": s.rate, "base_xp": s.base_xp, "max_xp": s.max_xp,
                    "engagement_window_mins": s.engagement_window_mins, "xp_cooldown_secs": s.xp_cooldown_secs,
                    "levelup_channel_id": id(s.levelup_channel_id),
                })
                .as_object()
                .cloned()
                .unwrap_or_default())
            },
            write: |data, guild, v| {
                let s = leveling::store::Settings {
                    rate: get_num(v, "rate"),
                    base_xp: get_num(v, "base_xp"),
                    max_xp: get_num(v, "max_xp"),
                    engagement_window_mins: get_num(v, "engagement_window_mins"),
                    xp_cooldown_secs: get_num(v, "xp_cooldown_secs"),
                    levelup_channel_id: get_id(v, "levelup_channel_id"),
                };
                s.validate()?;
                data.levels.lock().unwrap().save_settings(guild, s).map_err(db)
            },
        },
        Form {
            cog: "RaidProtection",
            about: "What counts as a raid and what happens during one, like /raid setup.",
            fields: vec![
                field("joins", "Joins that count as a raid", "This many joins…", Kind::Number { min: 2, max: 100, unit: "joins" }),
                field("seconds", "Within", "…within this long starts raid mode", Kind::Number { min: 5, max: 600, unit: "seconds" }),
                field("quiet_minutes", "Raid ends after", "Raid mode ends after this long without raid-speed joins", Kind::Number { min: 1, max: 1440, unit: "minutes" }),
                field("lock_channels", "Lock channels", "Lock text channels during a raid", Kind::Bool),
                field("raise_verification", "Raise verification", "Raise the verification level to High during a raid", Kind::Bool),
                field("joiner_action", "Accounts that join during a raid", "What happens to them", Kind::Choice(JOINER_ACTIONS)),
                field("timeout_minutes", "Timeout length", "How long joiners are timed out for, if that's what happens to them", Kind::Number { min: 1, max: 40320, unit: "minutes" }),
                optional("alert_channel", "Alert channel", "Where raid alerts go; the join & leave log, or the system channel, if not set", Kind::Channel),
            ],
            read: |data, guild| {
                let s = data.raid_protection.lock().unwrap().settings(guild).map_err(db)?;
                Ok(json!({
                    "joins": s.joins, "seconds": s.seconds, "quiet_minutes": s.quiet_minutes,
                    "lock_channels": s.lock_channels, "raise_verification": s.raise_verification,
                    "joiner_action": s.joiner_action.key(), "timeout_minutes": s.timeout_minutes,
                    "alert_channel": id(s.alert_channel),
                })
                .as_object()
                .cloned()
                .unwrap_or_default())
            },
            write: |data, guild, v| {
                let store = data.raid_protection.lock().unwrap();
                let mut s = store.settings(guild).map_err(db)?;
                s.joins = get_num(v, "joins") as u32;
                s.seconds = get_num(v, "seconds") as u32;
                s.quiet_minutes = get_num(v, "quiet_minutes") as u32;
                s.lock_channels = get_bool(v, "lock_channels");
                s.raise_verification = get_bool(v, "raise_verification");
                s.joiner_action = JoinerAction::from_key(get_str(v, "joiner_action"));
                s.timeout_minutes = get_num(v, "timeout_minutes") as u32;
                s.alert_channel = get_id(v, "alert_channel");
                store.set_settings(guild, &s).map_err(db)
            },
        },
        Form {
            cog: "SuspiciousAccounts",
            about: "When staff are alerted about suspicious accounts, like /suspicious setup.",
            fields: vec![
                field("threshold", "Alert threshold", "Staff are alerted when a member's score reaches this", Kind::Number { min: 1, max: 100, unit: "points" }),
                optional("alert_channel", "Alert channel", "Where alerts go", Kind::Channel),
                field("auto_quarantine", "Quarantine automatically", "Also quarantine members who reach the threshold", Kind::Bool),
            ],
            read: |data, guild| {
                let s = data.suspicious_accounts.lock().unwrap().settings(guild).map_err(db)?;
                Ok(json!({ "threshold": s.threshold, "alert_channel": id(s.alert_channel), "auto_quarantine": s.auto_quarantine })
                    .as_object()
                    .cloned()
                    .unwrap_or_default())
            },
            write: |data, guild, v| {
                let store = data.suspicious_accounts.lock().unwrap();
                let mut s = store.settings(guild).map_err(db)?;
                s.threshold = get_num(v, "threshold") as u32;
                s.alert_channel = get_id(v, "alert_channel");
                s.auto_quarantine = get_bool(v, "auto_quarantine");
                store.set_settings(guild, &s).map_err(db)
            },
        },
        Form {
            cog: "Quarantine",
            about: "Where quarantined members wait and are reviewed. Run /quarantine setup once to create the role and channel.",
            fields: vec![
                optional("role_id", "Quarantine role", "Given to quarantined members", Kind::Role),
                optional("channel_id", "Quarantine channel", "The only channel quarantined members can see", Kind::Channel),
                optional("review_channel_id", "Review channel", "Where staff review quarantined members", Kind::Channel),
                field("new_account_days", "Quarantine new accounts", "Accounts younger than this are quarantined when they join (0 = off)", Kind::Number { min: 0, max: 365, unit: "days" }),
            ],
            read: |data, guild| {
                let s = data.quarantine.lock().unwrap().settings(guild).map_err(db)?;
                Ok(json!({
                    "role_id": id(s.role_id), "channel_id": id(s.channel_id), "review_channel_id": id(s.review_channel_id),
                    "new_account_days": s.new_account_days.unwrap_or(0),
                })
                .as_object()
                .cloned()
                .unwrap_or_default())
            },
            write: |data, guild, v| {
                let store = data.quarantine.lock().unwrap();
                let mut s = store.settings(guild).map_err(db)?;
                s.role_id = get_id(v, "role_id");
                s.channel_id = get_id(v, "channel_id");
                s.review_channel_id = get_id(v, "review_channel_id");
                let days = get_num(v, "new_account_days") as u32;
                s.new_account_days = (days > 0).then_some(days);
                store.set_settings(guild, &s).map_err(db)
            },
        },
        Form {
            cog: "StaffPolicy",
            about: "What happens to staff who break the policy, like /staff-policy setup.",
            fields: vec![
                field("staff_roles", "Staff roles", "The roles taken away; every role the bot can take if none are picked", Kind::Roles),
                optional("demoted_role", "Demoted role", "Given to staff in place of the roles taken", Kind::Role),
                optional("log_channel", "Case channel", "Where cases are posted; the member log channel if not set", Kind::Channel),
            ],
            read: |data, guild| {
                let s = data.staff_policy.lock().unwrap().settings(guild).map_err(db)?;
                Ok(json!({ "staff_roles": ids(&s.staff_roles), "demoted_role": id(s.demoted_role), "log_channel": id(s.log_channel) })
                    .as_object()
                    .cloned()
                    .unwrap_or_default())
            },
            write: |data, guild, v| {
                let s = crate::database::repositories::staff_policy::Settings {
                    demoted_role: get_id(v, "demoted_role"),
                    log_channel: get_id(v, "log_channel"),
                    staff_roles: get_ids(v, "staff_roles"),
                };
                data.staff_policy.lock().unwrap().save_settings(guild, &s).map_err(db)
            },
        },
        Form {
            cog: "Suggestions",
            about: "Where suggestions are posted and where they go once handled, like /suggestion setup.",
            fields: vec![
                field("channel_id", "Suggestions channel", "Where new suggestions are posted", Kind::Channel),
                field("accepted_channel_id", "Accepted channel", "Where accepted suggestions go", Kind::Channel),
                field("declined_channel_id", "Declined channel", "Where declined suggestions go", Kind::Channel),
            ],
            read: |data, guild| {
                let s = data.suggestions.lock().unwrap().setup(guild).map_err(db)?;
                Ok(json!({
                    "channel_id": id(s.map(|s| s.channel_id)),
                    "accepted_channel_id": id(s.map(|s| s.accepted_channel_id)),
                    "declined_channel_id": id(s.map(|s| s.declined_channel_id)),
                })
                .as_object()
                .cloned()
                .unwrap_or_default())
            },
            write: |data, guild, v| {
                let (Some(channel_id), Some(accepted_channel_id), Some(declined_channel_id)) =
                    (get_id(v, "channel_id"), get_id(v, "accepted_channel_id"), get_id(v, "declined_channel_id"))
                else {
                    return Err("Pick all three channels".to_string());
                };
                let setup = crate::database::repositories::suggestions::Setup { channel_id, accepted_channel_id, declined_channel_id };
                data.suggestions.lock().unwrap().save_setup(guild, &setup).map_err(db)
            },
        },
        Form {
            cog: "Tickets",
            about: "Where tickets are opened and who handles them. Run /ticket-setup once to post the ticket panel.",
            fields: vec![
                field("category_id", "Ticket category", "New ticket channels are made here", Kind::Category),
                field("transcript_channel_id", "Transcript channel", "Where transcripts of closed tickets go", Kind::Channel),
                field("handler_role_id", "Handler role", "Staff who can see and handle tickets", Kind::Role),
            ],
            read: |data, guild| {
                let s = data.tickets.lock().unwrap().setup(guild).map_err(db)?;
                Ok(json!({
                    "category_id": id(s.as_ref().map(|s| s.category_id)),
                    "transcript_channel_id": id(s.as_ref().map(|s| s.transcript_channel_id)),
                    "handler_role_id": id(s.as_ref().map(|s| s.handler_role_id)),
                })
                .as_object()
                .cloned()
                .unwrap_or_default())
            },
            write: |data, guild, v| {
                let store = data.tickets.lock().unwrap();
                let Some(mut setup) = store.setup(guild).map_err(db)? else {
                    return Err("Run /ticket-setup first, so there's a ticket panel".to_string());
                };
                setup.category_id = get_id(v, "category_id").unwrap_or(setup.category_id);
                setup.transcript_channel_id = get_id(v, "transcript_channel_id").unwrap_or(setup.transcript_channel_id);
                setup.handler_role_id = get_id(v, "handler_role_id").unwrap_or(setup.handler_role_id);
                store.save_setup(guild, &setup).map_err(db)
            },
        },
        Form {
            cog: "Sticky",
            about: "How sticky messages move, like /sticky delay. Stick messages with /sticky stick in their channel.",
            fields: vec![field(
                "delay",
                "Delay",
                "How long a sticky waits once it's due before moving down (0 = straight away)",
                Kind::Number { min: 0, max: 300, unit: "seconds" },
            )],
            read: |data, guild| {
                let delay = data.sticky.lock().unwrap().delay(guild).map_err(db)?;
                Ok(json!({ "delay": delay }).as_object().cloned().unwrap_or_default())
            },
            write: |data, guild, v| data.sticky.lock().unwrap().set_delay(guild, get_num(v, "delay") as u32).map_err(db),
        },
        Form {
            cog: "OffLimits",
            about: "Channels the bot ignores completely, like /offlimit.",
            fields: vec![field("channels", "Off-limits channels", "The bot does nothing at all in these", Kind::Channels)],
            read: |data, guild| {
                let channels = data.offlimit.lock().unwrap().list(guild).map_err(db)?;
                Ok(json!({ "channels": ids(&channels) }).as_object().cloned().unwrap_or_default())
            },
            write: |data, guild, v| {
                let store = data.offlimit.lock().unwrap();
                let wanted = get_ids(v, "channels");
                let current = store.list(guild).map_err(db)?;
                for channel in current.iter().filter(|c| !wanted.contains(c)) {
                    store.remove(*channel).map_err(db)?;
                }
                for channel in wanted.iter().filter(|c| !current.contains(c)) {
                    store.add(guild, *channel).map_err(db)?;
                }
                Ok(())
            },
        },
    ]
}

pub fn form(cog: &str) -> Option<Form> {
    let cog = crate::features::normalize_cog_name(cog);
    forms().into_iter().find(|f| crate::features::normalize_cog_name(f.cog) == cog)
}

/// The fields as the page needs them.
pub fn schema(form: &Form) -> Value {
    let fields: Vec<Value> = form
        .fields
        .iter()
        .map(|f| {
            let mut v = json!({ "key": f.key, "label": f.label, "help": f.help, "optional": f.optional });
            let kind = match &f.kind {
                Kind::Channel => json!({ "type": "channel" }),
                Kind::Channels => json!({ "type": "channels" }),
                Kind::Category => json!({ "type": "category" }),
                Kind::Role => json!({ "type": "role" }),
                Kind::Roles => json!({ "type": "roles" }),
                Kind::Number { min, max, unit } => json!({ "type": "number", "min": min, "max": max, "unit": unit }),
                Kind::Bool => json!({ "type": "bool" }),
                Kind::Choice(options) => json!({ "type": "choice", "options": options.iter().map(|(k, l)| json!({ "key": k, "label": l })).collect::<Vec<_>>() }),
            };
            v.as_object_mut().unwrap().extend(kind.as_object().cloned().unwrap_or_default());
            v
        })
        .collect();
    json!({ "cog": form.cog, "about": form.about, "fields": fields })
}

fn is_text(kind: serenity::ChannelType) -> bool {
    matches!(kind, serenity::ChannelType::Text | serenity::ChannelType::News)
}

/// Checks the values sent for every field against the server, keeping only what's valid.
/// Returns the values to save, or what's wrong.
pub fn check(form: &Form, guild: &serenity::Guild, sent: &Value) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for f in &form.fields {
        let v = sent.get(f.key).unwrap_or(&Value::Null);
        let parse_id = |v: &Value| v.as_str().and_then(|s| s.parse::<u64>().ok());
        let channel = |id: u64, want_category: bool| {
            guild.channels.get(&serenity::ChannelId::new(id)).is_some_and(|c| {
                if want_category { c.kind == serenity::ChannelType::Category } else { is_text(c.kind) }
            })
        };
        let role = |id: u64| guild.roles.contains_key(&serenity::RoleId::new(id)) && id != guild.id.get();
        let checked = match &f.kind {
            Kind::Channel | Kind::Category | Kind::Role => {
                if v.is_null() || v.as_str() == Some("") {
                    if !f.optional {
                        return Err(format!("Pick a {}", f.label.to_lowercase()));
                    }
                    Value::Null
                } else {
                    let id = parse_id(v).ok_or_else(|| format!("{} isn't valid", f.label))?;
                    let ok = match f.kind {
                        Kind::Role => role(id),
                        Kind::Category => channel(id, true),
                        _ => channel(id, false),
                    };
                    if !ok {
                        return Err(format!("{}: that isn't in this server any more", f.label));
                    }
                    Value::String(id.to_string())
                }
            }
            Kind::Channels | Kind::Roles => {
                let list = v.as_array().ok_or_else(|| format!("{} isn't valid", f.label))?;
                let mut picked = Vec::new();
                for item in list {
                    let id = parse_id(item).ok_or_else(|| format!("{} isn't valid", f.label))?;
                    let ok = if matches!(f.kind, Kind::Roles) { role(id) } else { channel(id, false) };
                    if !ok {
                        return Err(format!("{}: one of them isn't in this server any more", f.label));
                    }
                    if !picked.contains(&id) {
                        picked.push(id);
                    }
                }
                ids(&picked)
            }
            Kind::Number { min, max, unit } => {
                let n = v.as_i64().ok_or_else(|| format!("{} needs a whole number", f.label))?;
                if !(*min..=*max).contains(&n) {
                    return Err(format!("{} must be between {min} and {max} {unit}", f.label));
                }
                json!(n)
            }
            Kind::Bool => Value::Bool(v.as_bool().ok_or_else(|| format!("{} needs to be on or off", f.label))?),
            Kind::Choice(options) => {
                let key = v.as_str().filter(|k| options.iter().any(|(o, _)| o == k)).ok_or_else(|| format!("Pick one of the options for {}", f.label))?;
                Value::String(key.to_string())
            }
        };
        out.insert(f.key.to_string(), checked);
    }
    Ok(out)
}

/// The labels of the fields whose values differ, for the audit log.
pub fn changed(form: &Form, before: &Map<String, Value>, after: &Map<String, Value>) -> Vec<&'static str> {
    form.fields.iter().filter(|f| before.get(f.key) != after.get(f.key)).map(|f| f.label).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_is_for_a_real_cog_and_reads_back_its_own_fields() {
        for form in forms() {
            assert!(crate::features::COGS.contains(&form.cog), "{} isn't a cog", form.cog);
            let schema = schema(&form);
            assert_eq!(schema["fields"].as_array().unwrap().len(), form.fields.len());
        }
        assert!(form("raidprotection").is_some());
        assert!(form("nope").is_none());
    }
}
