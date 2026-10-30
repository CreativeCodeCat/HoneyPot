//! Data types more than one feature uses.

use ::serenity::all as serenity;

/// A member as the image cards draw them: their names and avatar.
#[derive(Clone, Debug, Default)]
pub struct CardUser {
    /// Names to show, in order of preference; the first one the font can draw is used
    pub names: Vec<String>,
    /// PNG bytes of the avatar
    pub avatar: Option<Vec<u8>>,
}

impl CardUser {
    /// Where to download a user's avatar as a PNG. Discord serves PNGs for any avatar, and
    /// the card renderer only decodes PNG.
    pub fn avatar_url(user: &serenity::User) -> String {
        match &user.avatar {
            Some(hash) => format!("https://cdn.discordapp.com/avatars/{}/{hash}.png?size=256", user.id),
            None => user.default_avatar_url(),
        }
    }

    /// Downloads the user's avatar; `avatar` is `None` if it can't be.
    pub async fn fetch(names: Vec<String>, user: &serenity::User) -> Self {
        Self { names, avatar: crate::render::download(&Self::avatar_url(user)).await }
    }
}
