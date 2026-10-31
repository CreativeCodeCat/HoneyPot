//! Dashboard: a web page showing the bot's status and servers, served on `DASHBOARD_ADDR`
//! (default 127.0.0.1:8080) once the cache is ready. People sign in with Discord and see the
//! servers they own or have Administrator or Manage Server in; the bot's owners see them all.
//! It needs the bot's client secret (`DISCORD_CLIENT_SECRET`) and `DASHBOARD_URL`'s
//! `/callback` added as a redirect in the Discord developer portal.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::serenity::all as serenity;
use axum::extract::{Path, Query, State as AxumState};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{AppendHeaders, Html, IntoResponse, Redirect, Response};
use axum::{routing::get, Json, Router};
use rand::Rng;
use serde_json::{json, Value};

use super::framework::Command;
use crate::database::repositories::dashboard_log::Action as DashboardAction;
use crate::database::repositories::dashboard_sessions::token_hash;
use crate::features::{cog_manager, normalize_cog_name, Data, COGS, COG_MANAGER};
use crate::log;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_URL: &str = "http://localhost:8080";
const PAGE: &str = include_str!("../../assets/dashboard/index.html");
const API: &str = "https://discord.com/api/v10";
const COOKIE: &str = "hp_session";
/// Cookie holding the OAuth state while someone is signing in
const STATE_COOKIE: &str = "hp_state";
/// How long a sign-in lasts; the servers someone manages are read again when they sign in
const SESSION_LENGTH: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const ADMINISTRATOR: u64 = 0x8;
/// Administrator or Manage Server
const MANAGE_PERMISSIONS: u64 = ADMINISTRATOR | 0x20;
/// Changes listed on a server's Audit page
const AUDIT_SIZE: u32 = 50;

struct Session {
    user_id: u64,
    name: String,
    avatar: String,
    /// Servers they own or manage, and whether they can turn cogs on and off there (as owner
    /// or admin, like `/enablecog`); None for the bot's owners, who can do it everywhere
    guilds: Option<HashMap<u64, bool>>,
    /// The servers they own or manage, as Discord listed them when they signed in
    managed: Vec<ManagedGuild>,
    /// Unix seconds
    expires: i64,
}

impl Session {
    /// As saved in the sessions database.
    fn to_json(&self) -> Value {
        let guilds = self.guilds.as_ref().map(|g| g.iter().map(|(id, admin)| (id.to_string(), json!(admin))).collect::<serde_json::Map<_, _>>());
        let managed: Vec<Value> =
            self.managed.iter().map(|g| json!({ "id": g.id.to_string(), "name": g.name, "icon": g.icon, "admin": g.admin })).collect();
        json!({ "user_id": self.user_id.to_string(), "name": self.name, "avatar": self.avatar, "guilds": guilds, "managed": managed })
    }

    fn from_json(data: &Value, expires: i64) -> Option<Self> {
        let id = |v: &Value| v.as_str()?.parse::<u64>().ok();
        let guilds = match &data["guilds"] {
            Value::Null => None,
            Value::Object(map) => Some(map.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_bool()?))).collect()),
            _ => return None,
        };
        let managed = data["managed"]
            .as_array()?
            .iter()
            .filter_map(|g| {
                Some(ManagedGuild {
                    id: id(&g["id"])?,
                    name: g["name"].as_str()?.to_string(),
                    icon: g["icon"].as_str().map(str::to_string),
                    admin: g["admin"].as_bool()?,
                })
            })
            .collect();
        Some(Session {
            user_id: id(&data["user_id"])?,
            name: data["name"].as_str()?.to_string(),
            avatar: data["avatar"].as_str()?.to_string(),
            guilds,
            managed,
            expires,
        })
    }
}

#[derive(Clone)]
struct ManagedGuild {
    id: u64,
    name: String,
    icon: Option<String>,
    /// Owner or Administrator, so they can change settings and cogs
    admin: bool,
}

/// What the bot asks for when it's added to a server: what its features use
fn invite_permissions() -> serenity::Permissions {
    use serenity::Permissions as P;
    P::VIEW_CHANNEL
        | P::SEND_MESSAGES
        | P::SEND_MESSAGES_IN_THREADS
        | P::EMBED_LINKS
        | P::ATTACH_FILES
        | P::ADD_REACTIONS
        | P::USE_EXTERNAL_EMOJIS
        | P::READ_MESSAGE_HISTORY
        | P::MENTION_EVERYONE
        | P::MANAGE_MESSAGES
        | P::MANAGE_CHANNELS
        | P::MANAGE_ROLES
        | P::MANAGE_GUILD
        | P::VIEW_AUDIT_LOG
        | P::KICK_MEMBERS
        | P::BAN_MEMBERS
        | P::MODERATE_MEMBERS
        | P::CONNECT
}

/// Held while a toggle updates a server's slash commands.
static SYNCING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone)]
struct Dashboard {
    ctx: serenity::Context,
    data: Data,
    client_id: String,
    client_secret: String,
    /// Where people reach the dashboard, without a trailing slash
    url: String,
    owners: Arc<HashSet<u64>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    http: reqwest::Client,
    /// Every command anyone can use, as server pages list them; built once, as it doesn't change
    commands: Arc<Vec<CommandInfo>>,
    /// The bot's commands, for updating a server's slash commands when a cog is toggled
    registered: Arc<Vec<Command>>,
}

/// A command as the dashboard shows it: subcommands are listed on their own.
#[derive(Clone)]
struct CommandInfo {
    /// The cog's name as `COGS` has it, or "Other"
    cog: String,
    /// e.g. "/sticky stick <message> [count]", or a right-click menu name
    usage: String,
    description: String,
    permissions: Vec<&'static str>,
}

/// Every command people can run, leaving out the bot owner's.
fn command_infos(commands: &[Command]) -> Vec<CommandInfo> {
    // `qualified_name` is only filled in once the framework starts, so the name is built here
    fn walk(command: &Command, parent: &str, cog: &str, out: &mut Vec<CommandInfo>) {
        if command.owners_only {
            return;
        }
        let name = if parent.is_empty() { command.name.clone() } else { format!("{parent} {}", command.name) };
        if !command.subcommands.is_empty() {
            for sub in &command.subcommands {
                walk(sub, &name, cog, out);
            }
            return;
        }
        let usage = if let Some(menu) = &command.context_menu_name {
            format!("{menu} (right-click menu)")
        } else {
            let params: Vec<String> =
                command.parameters.iter().map(|p| if p.required { format!("<{}>", p.name) } else { format!("[{}]", p.name) }).collect();
            format!("/{name} {}", params.join(" ")).trim_end().to_string()
        };
        out.push(CommandInfo {
            cog: cog.to_string(),
            usage,
            description: command.description.clone().unwrap_or_default(),
            permissions: command.required_permissions.get_permission_names(),
        });
    }
    let mut out = Vec::new();
    for command in commands {
        let cog = command
            .category
            .as_deref()
            .and_then(|c| COGS.iter().find(|cog| normalize_cog_name(cog) == normalize_cog_name(c)))
            .copied()
            .unwrap_or("Other");
        walk(command, "", cog, &mut out);
    }
    out
}

pub async fn serve(ctx: serenity::Context, data: Data) {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let Some(client_secret) = env("DISCORD_CLIENT_SECRET") else {
        return log::warning("The dashboard is off: set DISCORD_CLIENT_SECRET in .env so people can sign in");
    };
    let info = match ctx.http.get_current_application_info().await {
        Ok(info) => info,
        Err(e) => return log::error(format!("Could not start the dashboard, as the bot's application couldn't be read: {e}")),
    };
    let mut owners: HashSet<u64> = info.owner.iter().map(|o| o.id.get()).collect();
    if let Some(team) = &info.team {
        owners.extend(team.members.iter().map(|m| m.user.id.get()));
    }
    let addr = env("DASHBOARD_ADDR").unwrap_or_else(|| DEFAULT_ADDR.to_string());
    let url = env("DASHBOARD_URL").unwrap_or_else(|| DEFAULT_URL.to_string()).trim_end_matches('/').to_string();
    let sessions = load_sessions(&data);
    let dash = Dashboard {
        ctx,
        data,
        client_id: info.id.get().to_string(),
        client_secret,
        url,
        owners: Arc::new(owners),
        sessions: Arc::new(Mutex::new(sessions)),
        http: reqwest::Client::new(),
        commands: Arc::new(command_infos(&super::command_router::commands())),
        registered: Arc::new(super::command_router::commands()),
    };
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        // The page shows a server itself, from the address
        .route("/servers/{id}", get(|| async { Html(PAGE) }))
        .route("/servers/{id}/{section}", get(|| async { Html(PAGE) }))
        .route("/servers/{id}/{section}/{item}", get(|| async { Html(PAGE) }))
        .route("/system", get(|| async { Html(PAGE) }))
        .route("/add", get(|| async { Html(PAGE) }))
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/logout", get(logout))
        .route("/api/overview", get(overview))
        .route("/api/servers/{id}", get(server))
        .route("/api/servers/{id}/cogs/{cog}", axum::routing::post(toggle_cog))
        .route("/api/servers/{id}/cogs/{cog}/data", get(cog_data))
        .route("/api/servers/{id}/activity", get(activity))
        .route("/api/servers/{id}/settings", get(settings).post(save_setting))
        .route("/api/servers/{id}/leaderboards", get(leaderboards))
        .route("/api/servers/{id}/config/{cog}", get(cog_config).post(save_cog_config))
        .route("/api/system", get(system))
        .route("/api/invite", get(invite))
        .route("/api/system/maintenance", axum::routing::post(set_maintenance))
        .route("/api/system/private", axum::routing::post(set_private))
        .with_state(dash.clone());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => return log::error(format!("Could not start the dashboard on {addr}: {e}")),
    };
    log::blue(format!("Dashboard running at {} (listening on {addr})", dash.url));
    if let Err(e) = axum::serve(listener, app).await {
        log::error(format!("The dashboard stopped: {e}"));
    }
}

/// The sign-ins saved before a restart that haven't expired.
fn load_sessions(data: &Data) -> HashMap<String, Session> {
    let saved = match data.dashboard_sessions.lock().unwrap().load(chrono::Utc::now().timestamp()) {
        Ok(saved) => saved,
        Err(e) => {
            log::error(format!("Error loading dashboard sign-ins: {e}"));
            return HashMap::new();
        }
    };
    saved
        .into_iter()
        .filter_map(|(hash, data, expires)| Some((hash, Session::from_json(&serde_json::from_str(&data).ok()?, expires)?)))
        .collect()
}

fn random_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('=').map(str::to_string))
}

impl Dashboard {
    /// A cookie only this site's pages send, and only over HTTPS if the dashboard uses it.
    fn set_cookie(&self, name: &str, value: &str, max_age: Duration) -> String {
        let secure = if self.url.starts_with("https://") { "; Secure" } else { "" };
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}", max_age.as_secs())
    }

    fn redirect_uri(&self) -> String {
        format!("{}/callback", self.url)
    }

    /// Who's signed in, dropping their session if it's expired. Sessions are kept by the
    /// hash of their token, as they're saved.
    fn session<T>(&self, headers: &HeaderMap, read: impl FnOnce(&Session) -> T) -> Option<T> {
        let hash = token_hash(&cookie(headers, COOKIE)?);
        let mut sessions = self.sessions.lock().unwrap();
        let expired = sessions.get(&hash).is_some_and(|s| s.expires <= chrono::Utc::now().timestamp());
        if expired {
            sessions.remove(&hash);
            if let Err(e) = self.data.dashboard_sessions.lock().unwrap().remove(&hash) {
                log::error(format!("Error removing an expired dashboard sign-in: {e}"));
            }
            return None;
        }
        let session = sessions.get(&hash)?;
        // In private mode only the bot's owners can use the dashboard. Others' sign-ins are
        // kept, and work again once it's off
        if crate::features::private_mode::enabled() && session.guilds.is_some() {
            return None;
        }
        Some(read(session))
    }

    async fn get_json(&self, path: &str, access_token: &str) -> Result<Value, String> {
        let res = self.http.get(format!("{API}{path}")).bearer_auth(access_token).send().await.map_err(|e| e.to_string())?;
        let status = res.status();
        let text = res.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// Swaps the code Discord sent back for the person's details and the servers they manage.
    async fn sign_in(&self, code: &str) -> Result<Session, String> {
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.redirect_uri()),
        ];
        let res = self.http.post(format!("{API}/oauth2/token")).form(&form).send().await.map_err(|e| e.to_string())?;
        let status = res.status();
        let text = res.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{status}: {text}"));
        }
        let token: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let access_token = token["access_token"].as_str().ok_or("no access token")?;

        let user = self.get_json("/users/@me", access_token).await?;
        let user_id: u64 = user["id"].as_str().and_then(|id| id.parse().ok()).ok_or("no user ID")?;
        let name = user["global_name"].as_str().or(user["username"].as_str()).unwrap_or("Unknown").to_string();
        let avatar = match user["avatar"].as_str() {
            Some(hash) => format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.png?size=64"),
            None => format!("https://cdn.discordapp.com/embed/avatars/{}.png", (user_id >> 22) % 6),
        };
        // Every server they own or manage, whether the bot is in it or not
        let list = self.get_json("/users/@me/guilds", access_token).await?;
        let managed: Vec<ManagedGuild> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let permissions: u64 = g["permissions"].as_str().and_then(|p| p.parse().ok()).unwrap_or(0);
                let owner = g["owner"].as_bool() == Some(true);
                if !owner && permissions & MANAGE_PERMISSIONS == 0 {
                    return None;
                }
                let id: u64 = g["id"].as_str()?.parse().ok()?;
                let icon = g["icon"].as_str().map(|hash| format!("https://cdn.discordapp.com/icons/{id}/{hash}.png?size=64"));
                Some(ManagedGuild { id, name: g["name"].as_str().unwrap_or("Unknown").to_string(), icon, admin: owner || permissions & ADMINISTRATOR != 0 })
            })
            .collect();
        let guilds = if self.owners.contains(&user_id) { None } else { Some(managed.iter().map(|g| (g.id, g.admin)).collect()) };
        let expires = chrono::Utc::now().timestamp() + SESSION_LENGTH.as_secs() as i64;
        Ok(Session { user_id, name, avatar, guilds, managed, expires })
    }
}

/// Sends the person to Discord to sign in.
async fn login(AxumState(dash): AxumState<Dashboard>) -> Response {
    let state = random_token();
    let query = [
        ("client_id", dash.client_id.as_str()),
        ("response_type", "code"),
        ("scope", "identify guilds"),
        ("redirect_uri", &dash.redirect_uri()),
        ("state", &state),
        ("prompt", "none"),
    ];
    let query: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    let location = format!("https://discord.com/oauth2/authorize?{}", query.join("&"));
    let cookie = dash.set_cookie(STATE_COOKIE, &state, Duration::from_secs(600));
    ([(header::SET_COOKIE, cookie)], Redirect::to(&location)).into_response()
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Where Discord sends the person back after they sign in.
async fn callback(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>) -> Response {
    // The state has to match the one set when they started, so no other site can sign them in
    let expected = cookie(&headers, STATE_COOKIE);
    if expected.is_none() || query.get("state") != expected.as_ref() {
        return (StatusCode::BAD_REQUEST, "This sign-in link has expired. Go back to the dashboard and sign in again.").into_response();
    }
    let Some(code) = query.get("code") else {
        return Redirect::to("/").into_response();
    };
    let session = match dash.sign_in(code).await {
        Ok(session) => session,
        Err(e) => {
            log::warning(format!("Dashboard sign-in failed: {e}"));
            return (StatusCode::BAD_GATEWAY, "Signing in with Discord failed. Go back to the dashboard and try again.").into_response();
        }
    };
    if crate::features::private_mode::enabled() && session.guilds.is_some() {
        log::warning(format!("[PRIVATE] Turned {} ({}) away from the dashboard: private mode is on", session.name, session.user_id));
        let clear = dash.set_cookie(STATE_COOKIE, "", Duration::ZERO);
        return ([(header::SET_COOKIE, clear)], Redirect::to("/?denied=private")).into_response();
    }
    log::info(format!("{} ({}) signed in to the dashboard", session.name, session.user_id));
    let token = random_token();
    let hash = token_hash(&token);
    if let Err(e) = dash.data.dashboard_sessions.lock().unwrap().save(&hash, session.user_id, &session.to_json().to_string(), session.expires) {
        // Still signed in until the bot restarts
        log::error(format!("Error saving a dashboard sign-in: {e}"));
    }
    dash.sessions.lock().unwrap().insert(hash, session);
    // Appended, as a plain array of headers would keep only the last Set-Cookie
    let cookies = AppendHeaders([
        (header::SET_COOKIE, dash.set_cookie(COOKIE, &token, SESSION_LENGTH)),
        (header::SET_COOKIE, dash.set_cookie(STATE_COOKIE, "", Duration::ZERO)),
    ]);
    (cookies, Redirect::to("/")).into_response()
}

async fn logout(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, COOKIE) {
        let hash = token_hash(&token);
        dash.sessions.lock().unwrap().remove(&hash);
        if let Err(e) = dash.data.dashboard_sessions.lock().unwrap().remove(&hash) {
            log::error(format!("Error removing a dashboard sign-in: {e}"));
        }
    }
    ([(header::SET_COOKIE, dash.set_cookie(COOKIE, "", Duration::ZERO))], Redirect::to("/")).into_response()
}

/// The bot and the servers the signed-in person can see, as the page shows them.
async fn overview(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap) -> Response {
    let Some((user, allowed)) = dash.session(&headers, |s| {
        (json!({ "id": s.user_id.to_string(), "name": s.name, "avatar": s.avatar, "owner": s.guilds.is_none() }), s.guilds.clone())
    }) else {
        return signed_out();
    };
    let cache = &dash.ctx.cache;
    let bot = cache.current_user().clone();
    let cogs: Vec<String> = COGS.iter().map(|cog| normalize_cog_name(cog)).collect();
    let mut guilds: Vec<Value> = cache
        .guilds()
        .iter()
        .filter(|id| allowed.as_ref().is_none_or(|allowed| allowed.contains_key(&id.get())))
        .filter_map(|id| {
            let guild = cache.guild(*id)?;
            let enabled = cogs.iter().filter(|cog| dash.data.is_cog_enabled(id.get(), cog)).count();
            Some(json!({
                "id": id.get().to_string(),
                "name": guild.name,
                "icon": guild.icon_url(),
                "members": guild.member_count,
                "channels": guild.channels.len(),
                "cogs_enabled": enabled,
            }))
        })
        .collect();
    guilds.sort_by_key(|g| std::cmp::Reverse(g["members"].as_u64().unwrap_or(0)));
    let members: u64 = guilds.iter().filter_map(|g| g["members"].as_u64()).sum();
    let channels: u64 = guilds.iter().filter_map(|g| g["channels"].as_u64()).sum();
    Json(json!({
        "user": user,
        "bot": {
            "name": bot.name,
            "avatar": bot.face(),
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": dash.data.start_time.elapsed().as_secs(),
            "shards": cache.shard_count(),
        },
        "totals": { "servers": guilds.len(), "members": members, "channels": channels, "cogs": cogs.len() },
        "guilds": guilds,
    }))
    .into_response()
}

/// One server's cogs, whether each is on, and their commands.
async fn server(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (guild_id, can_toggle) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let Some(guild) = dash.ctx.cache.guild(guild_id).map(|g| {
        json!({
            "id": guild_id.to_string(),
            "name": g.name,
            "icon": g.icon_url(),
            "members": g.member_count,
            "channels": g.channels.len(),
            "roles": g.roles.len(),
            "boosts": g.premium_subscription_count.unwrap_or(0),
            "boost_tier": u8::from(g.premium_tier),
            "owner_id": g.owner_id.get().to_string(),
            "owner_name": g.members.get(&g.owner_id).map(|m| m.display_name().to_string()),
            "created": g.id.created_at().unix_timestamp(),
        })
    }) else {
        // The bot left between checking and reading it
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "The bot isn't in that server" }))).into_response();
    };
    let mut cogs: Vec<&str> = COGS.to_vec();
    if dash.commands.iter().any(|c| c.cog == "Other") {
        cogs.push("Other");
    }
    let cogs: Vec<Value> = cogs
        .into_iter()
        .filter_map(|cog| {
            let commands: Vec<Value> = dash
                .commands
                .iter()
                .filter(|c| c.cog == cog)
                .map(|c| json!({ "usage": c.usage, "description": c.description, "permissions": c.permissions }))
                .collect();
            if commands.is_empty() {
                return None;
            }
            // Commands outside a cog are always on, as is the cog manager
            let enabled = cog == "Other" || dash.data.is_cog_enabled(guild_id, &normalize_cog_name(cog));
            let locked = cog == "Other" || normalize_cog_name(cog) == COG_MANAGER;
            let has_data = super::cog_data::has_data(cog, guild_id);
            let configurable = super::dashboard_config::form(cog).is_some();
            Some(json!({ "name": cog, "enabled": enabled, "locked": locked, "has_data": has_data, "configurable": configurable, "commands": commands }))
        })
        .collect();
    Json(json!({
        "guild": guild,
        "can_toggle": can_toggle,
        "cogs": cogs,
    }))
    .into_response()
}

impl Dashboard {
    /// The server a request is for, if the person signed in manages it and the bot is in it,
    /// with whether they can turn its cogs on and off. Otherwise, the response to send.
    fn access(&self, headers: &HeaderMap, id: &str) -> Result<(u64, bool), Response> {
        let Some(allowed) = self.session(headers, |s| s.guilds.clone()) else {
            return Err(signed_out());
        };
        let not_found = || (StatusCode::NOT_FOUND, Json(json!({ "error": "You don't manage that server, or the bot isn't in it" }))).into_response();
        let guild_id = id.parse::<u64>().ok().filter(|&id| id != 0).ok_or_else(not_found)?;
        let can_toggle = match &allowed {
            None => true,
            Some(allowed) => *allowed.get(&guild_id).ok_or_else(not_found)?,
        };
        if self.ctx.cache.guild(guild_id).is_none() {
            return Err(not_found());
        }
        Ok((guild_id, can_toggle))
    }
}

/// Turns a cog on or off in a server, like `/enablecog` and `/disablecog`, and updates the
/// server's slash commands to match in the background, as Discord can take a while. Takes `{"enabled": true}` or `{"enabled": false}`.
async fn toggle_cog(
    AxumState(dash): AxumState<Dashboard>,
    headers: HeaderMap,
    Path((id, cog)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let error = |status: StatusCode, text: &str| (status, Json(json!({ "error": text }))).into_response();
    let (guild_id, can_toggle) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    if !can_toggle {
        return error(StatusCode::FORBIDDEN, "Only the server's owner and admins can turn cogs on and off");
    }
    let Some(enabled) = body["enabled"].as_bool() else {
        return error(StatusCode::BAD_REQUEST, "Send {\"enabled\": true} or {\"enabled\": false}");
    };
    let Some(cog) = COGS.iter().map(|c| normalize_cog_name(c)).find(|c| *c == normalize_cog_name(&cog)) else {
        return error(StatusCode::NOT_FOUND, "There's no cog by that name");
    };
    if cog == COG_MANAGER {
        return error(StatusCode::BAD_REQUEST, "The cog manager can't be turned off");
    }
    if let Err(e) = dash.data.cog_config.lock().unwrap().set_enabled(guild_id, &cog, enabled) {
        log::error(format!("Error saving cog {cog} for guild {guild_id} from the dashboard: {e}"));
        return error(StatusCode::INTERNAL_SERVER_ERROR, "The change couldn't be saved");
    }
    let display = COGS.iter().find(|c| normalize_cog_name(c) == cog).copied().unwrap_or(&cog);
    dash.record(&headers, guild_id, format!("Turned {display} {}", if enabled { "on" } else { "off" }));
    let guild = serenity::GuildId::new(guild_id);
    tokio::spawn(async move {
        // One at a time, each reading the latest settings, so quick toggles can't finish out
        // of order and leave Discord with an older list
        let _syncing = SYNCING.lock().await;
        if let Err(e) = cog_manager::sync_guild_commands(&dash.ctx.http, &dash.registered, &dash.data, guild).await {
            log::error(format!("Error syncing commands for guild {guild_id}: {e}"));
        }
    });
    Json(json!({ "enabled": enabled })).into_response()
}

/// What a cog keeps in the database for a server: its tables, and a page of one of them
/// (`?table=<id>&page=<n>`, the first table with rows if none is given).
async fn cog_data(
    AxumState(dash): AxumState<Dashboard>,
    headers: HeaderMap,
    Path((id, cog)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (guild_id, _) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let Some(cog) = COGS.iter().find(|c| normalize_cog_name(c) == normalize_cog_name(&cog)) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "There's no cog by that name" }))).into_response();
    };
    let tables = super::cog_data::tables(cog, guild_id);
    let chosen = match query.get("table") {
        Some(id) => tables.iter().find(|t| &t.id == id),
        None => tables.iter().find(|t| t.rows > 0).or(tables.first()),
    };
    let page = query.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    let data = match chosen.map(|t| super::cog_data::page(t, guild_id, page)).transpose() {
        Ok(data) => data,
        Err(e) => {
            log::error(format!("Error reading {cog} data for guild {guild_id}: {e}"));
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "The data couldn't be read" }))).into_response();
        }
    };
    let list: Vec<Value> = tables.iter().map(|t| json!({ "id": t.id, "name": t.name, "description": t.description, "rows": t.rows })).collect();
    let names = data.as_ref().map(|d| dash.names(guild_id, &d["rows"])).unwrap_or_default();
    // Readable names for keys stored in a column, like "logs.joinleave" or "game:hack"
    let mut labels = serde_json::Map::new();
    match chosen.map(|t| (t.file, t.table.as_str())) {
        Some(("server_setup.db", "settings")) => {
            let keys = setting_keys().into_iter().map(|(key, label, _)| (key, Value::String(label)));
            labels.insert("key".to_string(), Value::Object(keys.collect()));
        }
        Some(("cog_config.db", "cog_config")) => {
            let mut cogs: serde_json::Map<String, Value> =
                COGS.iter().map(|c| (normalize_cog_name(c), Value::String(c.to_string()))).collect();
            for game in crate::features::games::GAMES {
                cogs.insert(format!("game:{game}"), Value::String(format!("{} (game)", crate::features::title_case(game))));
            }
            labels.insert("cog_name".to_string(), Value::Object(cogs));
        }
        _ => {}
    }
    let enabled = dash.data.is_cog_enabled(guild_id, &normalize_cog_name(cog));
    Json(json!({
        "cog": cog,
        "enabled": enabled,
        "tables": list,
        "table": chosen.map(|t| t.id.clone()),
        "data": data,
        "names": names,
        "labels": labels,
    }))
    .into_response()
}

impl Dashboard {
    /// Notes a change someone made on the dashboard, for the server's Audit page and the log.
    fn record(&self, headers: &HeaderMap, guild_id: u64, action: String) {
        let Some((user_id, user_name)) = self.session(headers, |s| (s.user_id, s.name.clone())) else { return };
        log::info(format!("{user_name} ({user_id}) in guild {guild_id} from the dashboard: {action}"));
        let entry = DashboardAction { user_id, user_name, action, at: chrono::Utc::now().timestamp() };
        if let Err(e) = self.data.dashboard_log.lock().unwrap().add(guild_id, &entry) {
            log::error(format!("Error saving a dashboard action for guild {guild_id}: {e}"));
        }
    }

    /// Avatar URLs for users, by ID, from the server's members or the users the bot knows.
    fn avatars<'a>(&self, guild_id: u64, users: impl Iterator<Item = &'a str>) -> serde_json::Map<String, Value> {
        let cache = &self.ctx.cache;
        let guild = cache.guild(guild_id);
        let mut avatars = serde_json::Map::new();
        for user in users.filter_map(|id| id.parse::<u64>().ok()).filter(|&id| id != 0) {
            let face = guild
                .as_ref()
                .and_then(|g| g.members.get(&serenity::UserId::new(user)).map(|m| m.face()))
                .or_else(|| cache.user(user).map(|u| u.face()));
            if let Some(face) = face {
                avatars.insert(user.to_string(), Value::String(face));
            }
        }
        avatars
    }

    fn is_owner(&self, headers: &HeaderMap) -> Option<bool> {
        self.session(headers, |s| s.guilds.is_none())
    }
}

/// For anyone not signed in, saying whether that's because private mode is on.
fn signed_out() -> Response {
    let private = crate::features::private_mode::enabled();
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Sign in first", "private": private }))).into_response()
}

fn json_error(status: StatusCode, text: &str) -> Response {
    (status, Json(json!({ "error": text }))).into_response()
}

/// The server's security status, recent catches and incidents, and changes made here.
async fn activity(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (guild_id, _) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let security = super::dashboard_stats::security(guild_id);
    let audit: Vec<Value> = match dash.data.dashboard_log.lock().unwrap().list(guild_id, AUDIT_SIZE) {
        Ok(actions) => actions
            .into_iter()
            .map(|a| json!({ "user_id": a.user_id.to_string(), "user_name": a.user_name, "action": a.action, "at": a.at }))
            .collect(),
        Err(e) => {
            log::error(format!("Error reading dashboard actions for guild {guild_id}: {e}"));
            Vec::new()
        }
    };
    let names = dash.names(guild_id, &security);
    let catchers = security["honeypot"]["recent"].as_array().into_iter().flatten().filter_map(|c| c["user_id"].as_str());
    let avatars = dash.avatars(guild_id, catchers);
    Json(json!({ "security": security, "audit": audit, "names": names, "avatars": avatars })).into_response()
}

/// The channels a server can pick in Settings: what each is for, by key.
fn setting_keys() -> Vec<(String, String, &'static str)> {
    use crate::core::framework::Choice;
    use crate::features::server_setup;
    let mut keys = vec![
        ("channel.commands".to_string(), "Commands channel".to_string(), "Channels"),
        ("channel.games".to_string(), "Games channel".to_string(), "Channels"),
        ("channel.announcements".to_string(), "Announcements channel".to_string(), "Channels"),
    ];
    keys.extend(server_setup::LOG_TYPES.iter().map(|&log| (server_setup::log_key(log), format!("{} log", log.name()), "Logs")));
    keys
}

/// The server's text channels, and the channel picked for each setting.
async fn settings(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (guild_id, can_edit) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let channels: Vec<Value> = {
        let Some(guild) = dash.ctx.cache.guild(guild_id) else { return json_error(StatusCode::NOT_FOUND, "The bot isn't in that server") };
        let category = |c: &serenity::GuildChannel| c.parent_id.and_then(|p| guild.channels.get(&p)).map(|p| (p.position, p.name.clone()));
        let mut text: Vec<&serenity::GuildChannel> = guild
            .channels
            .values()
            .filter(|c| matches!(c.kind, serenity::ChannelType::Text | serenity::ChannelType::News))
            .collect();
        text.sort_by_key(|c| (category(c).map(|(p, _)| p), c.position, c.id));
        text.into_iter()
            .map(|c| json!({ "id": c.id.get().to_string(), "name": c.name, "category": category(c).map(|(_, name)| name) }))
            .collect()
    };
    let saved: HashMap<String, u64> = match dash.data.server_setup.lock().unwrap().all(guild_id) {
        Ok(all) => all.into_iter().collect(),
        Err(e) => {
            log::error(format!("Error reading settings for guild {guild_id}: {e}"));
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "The settings couldn't be read");
        }
    };
    let settings: Vec<Value> = setting_keys()
        .into_iter()
        .map(|(key, label, group)| json!({ "key": key, "label": label, "group": group, "channel_id": saved.get(&key).map(|c| c.to_string()) }))
        .collect();
    Json(json!({ "can_edit": can_edit, "channels": channels, "settings": settings })).into_response()
}

/// Picks the channel for a setting, like `/server-setup`. Takes `{"key": "logs.message",
/// "channel_id": "123"}`, or a null `channel_id` to clear it.
async fn save_setting(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<Value>) -> Response {
    let (guild_id, can_edit) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    if !can_edit {
        return json_error(StatusCode::FORBIDDEN, "Only the server's owner and admins can change settings");
    }
    let Some((key, label, _)) = setting_keys().into_iter().find(|(key, _, _)| Some(key.as_str()) == body["key"].as_str()) else {
        return json_error(StatusCode::BAD_REQUEST, "There's no setting by that name");
    };
    let channel: Option<u64> = match &body["channel_id"] {
        Value::Null => None,
        Value::String(id) => match id.parse() {
            Ok(id) => Some(id),
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "That isn't a channel"),
        },
        _ => return json_error(StatusCode::BAD_REQUEST, "Send a channel_id, or null to clear it"),
    };
    let channel_name = match channel {
        None => None,
        Some(channel) => {
            let name = dash.ctx.cache.guild(guild_id).and_then(|g| {
                g.channels
                    .get(&serenity::ChannelId::new(channel))
                    .filter(|c| matches!(c.kind, serenity::ChannelType::Text | serenity::ChannelType::News))
                    .map(|c| c.name.clone())
            });
            match name {
                Some(name) => Some(name),
                None => return json_error(StatusCode::BAD_REQUEST, "That isn't a text channel in this server"),
            }
        }
    };
    let store = dash.data.server_setup.lock().unwrap();
    let saved = match channel {
        Some(channel) => store.set(guild_id, &key, channel),
        None => store.remove(guild_id, &key).map(|_| ()),
    };
    drop(store);
    if let Err(e) = saved {
        log::error(format!("Error saving {key} for guild {guild_id} from the dashboard: {e}"));
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "The change couldn't be saved");
    }
    let action = match &channel_name {
        Some(name) => format!("Set the {} to #{name}", label.to_lowercase()),
        None => format!("Cleared the {}", label.to_lowercase()),
    };
    dash.record(&headers, guild_id, action);
    Json(json!({ "key": key, "channel_id": channel.map(|c| c.to_string()) })).into_response()
}

/// What a server can pick from in cog settings: its text channels, categories and roles.
fn pickers(guild: &serenity::Guild) -> Value {
    let category = |c: &serenity::GuildChannel| c.parent_id.and_then(|p| guild.channels.get(&p)).map(|p| (p.position, p.name.clone()));
    let mut text: Vec<&serenity::GuildChannel> =
        guild.channels.values().filter(|c| matches!(c.kind, serenity::ChannelType::Text | serenity::ChannelType::News)).collect();
    text.sort_by_key(|c| (category(c).map(|(p, _)| p), c.position, c.id));
    let channels: Vec<Value> =
        text.into_iter().map(|c| json!({ "id": c.id.get().to_string(), "name": c.name, "category": category(c).map(|(_, name)| name) })).collect();
    let mut cats: Vec<&serenity::GuildChannel> = guild.channels.values().filter(|c| c.kind == serenity::ChannelType::Category).collect();
    cats.sort_by_key(|c| (c.position, c.id));
    let categories: Vec<Value> = cats.into_iter().map(|c| json!({ "id": c.id.get().to_string(), "name": c.name })).collect();
    // Highest first, as Discord lists them, leaving out @everyone and bots' own roles
    let mut list: Vec<&serenity::Role> = guild.roles.values().filter(|r| r.id.get() != guild.id.get() && !r.managed).collect();
    list.sort_by_key(|r| std::cmp::Reverse(r.position));
    let roles: Vec<Value> = list.into_iter().map(|r| json!({ "id": r.id.get().to_string(), "name": r.name, "colour": r.colour.0 })).collect();
    json!({ "channels": channels, "categories": categories, "roles": roles })
}

/// A cog's settings form: its fields, their values, and what can be picked.
async fn cog_config(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path((id, cog)): Path<(String, String)>) -> Response {
    let (guild_id, can_edit) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let Some(form) = super::dashboard_config::form(&cog) else {
        return json_error(StatusCode::NOT_FOUND, "That cog has no settings to change here");
    };
    let values = match (form.read)(&dash.data, guild_id) {
        Ok(values) => values,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let Some(pickers) = dash.ctx.cache.guild(guild_id).map(|g| pickers(&g)) else {
        return json_error(StatusCode::NOT_FOUND, "The bot isn't in that server");
    };
    let enabled = dash.data.is_cog_enabled(guild_id, &normalize_cog_name(form.cog));
    let mut out = super::dashboard_config::schema(&form);
    let extra = json!({ "values": values, "can_edit": can_edit, "enabled": enabled, "pickers": pickers });
    out.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    Json(out).into_response()
}

/// Saves a cog's settings. Takes `{"values": {field: value, ...}}` with every field.
async fn save_cog_config(
    AxumState(dash): AxumState<Dashboard>,
    headers: HeaderMap,
    Path((id, cog)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let (guild_id, can_edit) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    if !can_edit {
        return json_error(StatusCode::FORBIDDEN, "Only the server's owner and admins can change settings");
    }
    let Some(form) = super::dashboard_config::form(&cog) else {
        return json_error(StatusCode::NOT_FOUND, "That cog has no settings to change here");
    };
    let checked = {
        let Some(guild) = dash.ctx.cache.guild(guild_id) else { return json_error(StatusCode::NOT_FOUND, "The bot isn't in that server") };
        super::dashboard_config::check(&form, &guild, &body["values"])
    };
    let values = match checked {
        Ok(values) => values,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e),
    };
    let before = (form.read)(&dash.data, guild_id).unwrap_or_default();
    if let Err(e) = (form.write)(&dash.data, guild_id, &values) {
        return json_error(StatusCode::BAD_REQUEST, &e);
    }
    let changed = super::dashboard_config::changed(&form, &before, &values);
    if !changed.is_empty() {
        dash.record(&headers, guild_id, format!("Changed {} settings: {}", form.cog, changed.join(", ").to_lowercase()));
    }
    let values = (form.read)(&dash.data, guild_id).unwrap_or(values);
    Json(json!({ "values": values, "changed": changed })).into_response()
}

/// The server's top members by level and by money.
async fn leaderboards(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (guild_id, _) = match dash.access(&headers, &id) {
        Ok(access) => access,
        Err(response) => return response,
    };
    let boards = super::dashboard_stats::leaderboards(guild_id);
    let names = dash.names(guild_id, &boards);
    let members = boards["levels"].as_array().into_iter().flatten().chain(boards["economy"].as_array().into_iter().flatten());
    let avatars = dash.avatars(guild_id, members.filter_map(|m| m["user_id"].as_str()));
    Json(json!({ "levels": boards["levels"], "economy": boards["economy"], "names": names, "avatars": avatars })).into_response()
}

/// The bot's host and every server it's in, for its owners.
async fn system(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap) -> Response {
    match dash.is_owner(&headers) {
        None => return signed_out(),
        Some(false) => return json_error(StatusCode::FORBIDDEN, "Only the bot's owners can see this"),
        Some(true) => {}
    }
    use crate::features::info;
    let cpu = info::cpu_percent().await;
    let memory = info::system_memory();
    let (db_files, db_size) = info::database_files();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|r| r.trim().to_string()).ok();
    let host = std::fs::read_to_string("/etc/hostname").map(|r| r.trim().to_string()).ok();
    let cache = &dash.ctx.cache;
    let mut guilds: Vec<Value> = cache
        .guilds()
        .iter()
        .filter_map(|id| {
            let g = cache.guild(*id)?;
            Some(json!({
                "id": id.get().to_string(),
                "name": g.name,
                "icon": g.icon_url(),
                "members": g.member_count,
                "owner": g.members.get(&g.owner_id).map(|m| m.user.name.clone()).unwrap_or_else(|| g.owner_id.to_string()),
                "joined": g.joined_at.unix_timestamp(),
            }))
        })
        .collect();
    guilds.sort_by_key(|g| std::cmp::Reverse(g["members"].as_u64().unwrap_or(0)));
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": dash.data.start_time.elapsed().as_secs(),
        "cpu_percent": cpu,
        "memory": memory.map(|(total, available)| json!({ "total": total, "used": total - available })),
        "bot_memory": info::bot_memory(),
        "database": { "files": db_files, "bytes": db_size },
        "kernel": kernel,
        "host": host,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "commands": info::count_commands(&dash.registered),
        "shards": cache.shard_count(),
        "users_cached": cache.user_count(),
        "guilds": guilds,
        "maintenance": crate::features::developer::maintenance(),
        "private": crate::features::private_mode::enabled(),
        "private_would_leave": dash.private_would_leave(),
    }))
    .into_response()
}

impl Dashboard {
    fn owner_ids(&self) -> HashSet<serenity::UserId> {
        self.owners.iter().map(|&id| serenity::UserId::new(id)).collect()
    }

    /// The servers private mode would make the bot leave.
    fn private_would_leave(&self) -> Vec<Value> {
        crate::features::private_mode::others(&self.ctx.cache, &self.owner_ids())
            .into_iter()
            .map(|(id, name)| json!({ "id": id.get().to_string(), "name": name }))
            .collect()
    }

    /// The bot owner's name, for the log, or the response to send anyone else.
    fn require_owner(&self, headers: &HeaderMap) -> Result<String, Response> {
        match self.session(headers, |s| (s.guilds.is_none(), format!("{} ({})", s.name, s.user_id))) {
            None => Err(signed_out()),
            Some((false, _)) => Err(json_error(StatusCode::FORBIDDEN, "Only the bot's owners can do this")),
            Some((true, who)) => Ok(who),
        }
    }
}

/// Turns maintenance mode on or off, like `/maintenance`. Takes `{"enabled": bool}`.
async fn set_maintenance(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let who = match dash.require_owner(&headers) {
        Ok(who) => who,
        Err(response) => return response,
    };
    let Some(on) = body["enabled"].as_bool() else {
        return json_error(StatusCode::BAD_REQUEST, "Send {\"enabled\": true} or {\"enabled\": false}");
    };
    if let Err(e) = crate::features::developer::set_maintenance(&dash.ctx, on) {
        log::error(format!("Error setting maintenance mode from the dashboard: {e}"));
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "The change couldn't be saved");
    }
    log::info(format!("{who} turned maintenance mode {} from the dashboard", if on { "on" } else { "off" }));
    Json(json!({ "enabled": on })).into_response()
}

/// Turns private mode on or off, like `/private`: turning it on leaves every server not
/// owned by a bot owner, so the page asks first. Takes `{"enabled": bool}`.
async fn set_private(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    use crate::features::private_mode;
    let who = match dash.require_owner(&headers) {
        Ok(who) => who,
        Err(response) => return response,
    };
    let Some(on) = body["enabled"].as_bool() else {
        return json_error(StatusCode::BAD_REQUEST, "Send {\"enabled\": true} or {\"enabled\": false}");
    };
    // Without knowing who the owners are, every server would look like someone else's
    if on && dash.owners.is_empty() {
        return json_error(StatusCode::CONFLICT, "The bot doesn't know who its owners are, so it can't tell which servers to keep");
    }
    if let Err(e) = private_mode::set(on) {
        log::error(format!("Error setting private mode from the dashboard: {e}"));
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "The change couldn't be saved");
    }
    if !on {
        log::info(format!("[PRIVATE] {who} turned private mode off from the dashboard"));
        return Json(json!({ "enabled": false, "left": 0, "failed": [] })).into_response();
    }
    let others = private_mode::others(&dash.ctx.cache, &dash.owner_ids());
    log::info(format!("[PRIVATE] {who} turned private mode on from the dashboard, leaving {} server(s)", others.len()));
    let failed = private_mode::leave(&dash.ctx.http, &others).await;
    let failed: Vec<Value> = failed.into_iter().map(|(name, error)| json!({ "name": name, "error": error })).collect();
    Json(json!({ "enabled": true, "left": others.len() - failed.len(), "failed": failed })).into_response()
}

impl Dashboard {
    /// The link that adds the bot to a server, with that server picked if given.
    fn invite_url(&self, guild_id: Option<u64>) -> String {
        let mut url = format!(
            "https://discord.com/oauth2/authorize?client_id={}&scope=bot%20applications.commands&permissions={}",
            self.client_id,
            invite_permissions().bits()
        );
        if let Some(guild_id) = guild_id {
            url.push_str(&format!("&guild_id={guild_id}&disable_guild_select=true"));
        }
        url
    }
}

/// The servers the signed-in person manages that the bot isn't in yet, with links to add it.
/// Manage Server is what Discord needs to add a bot, and everyone here has it.
async fn invite(AxumState(dash): AxumState<Dashboard>, headers: HeaderMap) -> Response {
    let Some(managed) = dash.session(&headers, |s| s.managed.clone()) else {
        return signed_out();
    };
    let cache = &dash.ctx.cache;
    let mut servers: Vec<Value> = managed
        .iter()
        .filter(|g| cache.guild(g.id).is_none())
        .map(|g| json!({ "id": g.id.to_string(), "name": g.name, "icon": g.icon, "invite": dash.invite_url(Some(g.id)) }))
        .collect();
    servers.sort_by_key(|g| g["name"].as_str().unwrap_or("").to_lowercase());
    let permissions = invite_permissions().get_permission_names();
    let private = crate::features::private_mode::enabled();
    Json(json!({ "invite": dash.invite_url(None), "servers": servers, "permissions": permissions, "private": private })).into_response()
}

/// Discord IDs: 17 to 20 digits.
static ID: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\b\d{17,20}\b").unwrap());

impl Dashboard {
    /// Names for the Discord IDs anywhere in `value`, from what the bot knows about the
    /// server: `#channel`, `@role`, members' display names, other users' names and the server
    /// itself. IDs it doesn't know (like message IDs) are left out.
    fn names(&self, guild_id: u64, value: &Value) -> serde_json::Map<String, Value> {
        fn collect(value: &Value, ids: &mut HashSet<u64>) {
            match value {
                Value::String(text) => ids.extend(ID.find_iter(text).filter_map(|m| m.as_str().parse::<u64>().ok())),
                Value::Number(n) => ids.extend(ID.find_iter(&n.to_string()).filter_map(|m| m.as_str().parse::<u64>().ok())),
                Value::Array(items) => items.iter().for_each(|v| collect(v, ids)),
                Value::Object(map) => map.values().for_each(|v| collect(v, ids)),
                _ => {}
            }
        }
        let mut ids = HashSet::new();
        collect(value, &mut ids);
        let mut names = serde_json::Map::new();
        let Some(guild) = self.ctx.cache.guild(guild_id) else { return names };
        for id in ids {
            let name = if id == guild_id {
                Some(guild.name.clone())
            } else if let Some(channel) = guild.channels.get(&serenity::ChannelId::new(id)) {
                Some(format!("#{}", channel.name))
            } else if let Some(thread) = guild.threads.iter().find(|t| t.id.get() == id) {
                Some(format!("#{}", thread.name))
            } else if let Some(role) = guild.roles.get(&serenity::RoleId::new(id)) {
                Some(format!("@{}", role.name))
            } else if let Some(member) = guild.members.get(&serenity::UserId::new(id)) {
                Some(format!("@{}", member.display_name()))
            } else {
                self.ctx.cache.user(id).map(|user| format!("@{}", user.display_name()))
            };
            if let Some(name) = name {
                names.insert(id.to_string(), Value::String(name));
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_read_by_name() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "a=1; hp_session=abc; hp_state=xyz".parse().unwrap());
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, STATE_COOKIE).as_deref(), Some("xyz"));
        assert_eq!(cookie(&headers, "hp"), None);
    }

    #[test]
    fn every_command_people_can_run_is_listed() {
        let infos = command_infos(&crate::core::command_router::commands());
        assert!(infos.iter().any(|c| c.cog == "Sticky" && c.usage == "/sticky stick <message> [count]"));
        assert!(infos.iter().any(|c| c.usage.starts_with("/reaction-roles add <message> <emoji> <role>")));
        // The bot owner's commands aren't shown
        assert!(!infos.iter().any(|c| c.usage.starts_with("/blacklist")));
    }

    #[test]
    fn the_bot_asks_for_what_it_uses_but_not_administrator() {
        let p = invite_permissions();
        assert!(!p.administrator());
        assert!(p.ban_members() && p.manage_roles() && p.manage_channels() && p.view_audit_log() && p.add_reactions());
    }

    #[test]
    fn saved_sessions_come_back_the_same() {
        let session = Session {
            user_id: 861270236475817994,
            name: "Wayne".into(),
            avatar: "https://cdn.discordapp.com/a.png".into(),
            guilds: Some(HashMap::from([(1162534077681913887, true), (945963538474754058, false)])),
            managed: vec![ManagedGuild { id: 945963538474754058, name: "Hub".into(), icon: None, admin: false }],
            expires: 1_900_000_000,
        };
        let back = Session::from_json(&session.to_json(), session.expires).unwrap();
        assert_eq!(back.user_id, session.user_id);
        assert_eq!(back.guilds, session.guilds);
        assert_eq!(back.managed.len(), 1);
        assert_eq!((back.managed[0].id, back.managed[0].admin), (945963538474754058, false));
        // A bot owner sees every server
        let owner = Session { guilds: None, ..session };
        assert!(Session::from_json(&owner.to_json(), 0).unwrap().guilds.is_none());
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(urlencode("http://localhost:8080/callback"), "http%3A%2F%2Flocalhost%3A8080%2Fcallback");
        assert_eq!(urlencode("identify guilds"), "identify%20guilds");
    }
}
