//! The bot's shared state: one handle to every store, given to each command and event.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::features::*;

pub type Data = Arc<State>;

pub struct State {
    pub start_time: Instant,
    pub cog_config: Mutex<crate::database::guild_config::CogConfig>,
    pub afk: Mutex<afk::Store>,
    pub applications: Mutex<applications::Store>,
    pub blacklist: Mutex<developer::Blacklist>,
    pub cooldowns: Mutex<cooldowns::Cooldowns>,
    pub counting: Mutex<games::counting::Store>,
    pub economy: Mutex<economy::store::Store>,
    pub guessing: Mutex<games::guessing::Store>,
    pub honeypot: Mutex<honeypot::store::Store>,
    pub levels: Mutex<leveling::store::Store>,
    pub lockdown: Mutex<lockdown::Store>,
    pub pets: Mutex<pets::store::Store>,
    pub profiles: Mutex<profile::Store>,
    pub rules: Mutex<rules::Store>,
    pub server_setup: Mutex<server_setup::Store>,
    pub staff_policy: Mutex<staff_policy::Store>,
    pub sticky: Mutex<sticky::Store>,
    pub suggestions: Mutex<suggestions::Store>,
    pub tickets: Mutex<tickets::store::Store>,
}

impl State {
    pub fn is_cog_enabled(&self, guild_id: u64, cog_name: &str) -> bool {
        self.cog_config
            .lock()
            .unwrap()
            .is_enabled(guild_id, cog_name)
    }
}
