//! Signed links to the verification page of the panel (`web` mode).
//!
//! The bot gives each member who clicks Verify a link that only works for
//! them, on that server, for a few minutes:
//! `<guild>.<user>.<expires>.<signature>`, the signature being an HMAC of
//! the rest keyed with the bot's token. The bot and the panel both know the
//! token, so no other secret has to be shared.

use crate::crypto::{hmac_sha256_hex, same};

/// How long a link works.
pub const LIFETIME_SECS: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub guild_id: String,
    pub user_id: String,
    pub expires: i64,
}

fn signature(token: &str, slug: &str, guild: &str, user: &str, expires: i64) -> String {
    let message = format!("verify:{slug}:{guild}:{user}:{expires}");
    hmac_sha256_hex(
        format!("dc-verify-link:{token}").as_bytes(),
        message.as_bytes(),
    )
}

/// Link value for a member, valid from `now` for [`LIFETIME_SECS`].
pub fn sign(token: &str, slug: &str, guild: &str, user: &str, now: i64) -> String {
    let expires = now + LIFETIME_SECS;
    let sig = signature(token, slug, guild, user, expires);
    format!("{guild}.{user}.{expires}.{sig}")
}

/// Checks a link value; `None` when it is forged, malformed or expired.
pub fn check(token: &str, slug: &str, value: &str, now: i64) -> Option<Claims> {
    let mut parts = value.split('.');
    let (guild, user, expires, sig) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !crate::is_snowflake(guild) || !crate::is_snowflake(user) {
        return None;
    }
    let expires: i64 = expires.parse().ok()?;
    if expires < now || !same(sig, &signature(token, slug, guild, user, expires)) {
        return None;
    }
    Some(Claims {
        guild_id: guild.to_owned(),
        user_id: user.to_owned(),
        expires,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: &str = "175928847299117063";
    const USER: &str = "175928847299117064";

    #[test]
    fn links_are_bound_to_bot_member_and_time() {
        let link = sign("token", "sang", GUILD, USER, 1000);
        let claims = check("token", "sang", &link, 1000).unwrap();
        assert_eq!(
            (claims.guild_id.as_str(), claims.user_id.as_str()),
            (GUILD, USER)
        );

        assert!(check("other-token", "sang", &link, 1000).is_none());
        assert!(check("token", "elyzium", &link, 1000).is_none());
        assert!(check("token", "sang", &link, 1000 + LIFETIME_SECS + 1).is_none());
        // Another member cannot reuse it.
        let stolen = link.replacen(USER, "175928847299117065", 1);
        assert!(check("token", "sang", &stolen, 1000).is_none());
        assert!(check("token", "sang", "1.2.3", 1000).is_none());
        assert!(check("token", "sang", &format!("{link}.x"), 1000).is_none());
    }
}
