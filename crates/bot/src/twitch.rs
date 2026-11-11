//! Twitch live alerts (ported from the Scanline bot). The bot's Twitch app
//! (`TWITCH_CLIENT_ID` / `TWITCH_CLIENT_SECRET` in its `.env`, or
//! `..._<SLUG>`) checks the streamers of every server once a minute.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    app::{App, str_of},
    ui::{self, Msg},
};

const POLL: Duration = Duration::from_secs(60);

struct Client {
    id: String,
    secret: String,
    token: Option<(String, Instant)>,
}

impl Client {
    async fn token(&mut self, app: &App) -> anyhow::Result<String> {
        if let Some((token, expires)) = &self.token
            && Instant::now() + Duration::from_secs(60) < *expires
        {
            return Ok(token.clone());
        }
        let data: Value = app
            .web
            .post("https://id.twitch.tv/oauth2/token")
            .query(&[
                ("client_id", self.id.as_str()),
                ("client_secret", self.secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let token = data["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no Twitch token"))?
            .to_owned();
        let lifetime = data["expires_in"].as_u64().unwrap_or(3600);
        self.token = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(token)
    }

    /// The live streams among `logins` (100 at most per call).
    async fn live(
        &mut self,
        app: &App,
        logins: &[String],
    ) -> anyhow::Result<HashMap<String, Value>> {
        let token = self.token(app).await?;
        let query: Vec<(&str, &str)> = logins
            .iter()
            .map(|login| ("user_login", login.as_str()))
            .collect();
        let response = app
            .web
            .get("https://api.twitch.tv/helix/streams")
            .query(&query)
            .header("Client-Id", &self.id)
            .bearer_auth(&token)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.token = None;
        }
        let data: Value = response.error_for_status()?.json().await?;
        Ok(data["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|stream| (str_of(stream, "user_login").to_lowercase(), stream.clone()))
            .collect())
    }
}

pub fn alert(login: &str, stream: &Value, role: Option<&str>) -> Msg {
    let title = str_of(stream, "title");
    let title = if title.is_empty() {
        "Come and watch 👀"
    } else {
        title
    };
    let name = str_of(stream, "user_name");
    let name = if name.is_empty() { login } else { name };
    let game = str_of(stream, "game_name");
    let mut text = format!(
        "### 🔴 {} is live on Twitch!\n{}",
        ui::escape(name),
        ui::escape(title)
    );
    if !game.is_empty() {
        text.push_str(&format!("\n-# {}", ui::escape(game)));
    }
    let thumbnail = str_of(stream, "thumbnail_url")
        .replace("{width}", "640")
        .replace("{height}", "360");
    let mut children = Vec::new();
    if let Some(role) = role {
        children.push(ui::text(format!("<@&{role}>")));
    }
    children.push(ui::text(text));
    if !thumbnail.is_empty() {
        // A changing query string, so Discord does not show a cached frame.
        children.push(ui::gallery(&format!("{thumbnail}?t={}", dc_core::now())));
    }
    children.push(ui::row(vec![ui::link_button(
        "Watch the stream",
        &format!("https://twitch.tv/{login}"),
    )]));
    let mut msg = Msg::card(0x9146FF, children);
    msg.ping_roles = role.map(str::to_owned).into_iter().collect();
    msg
}

/// Background task; does nothing without the bot's Twitch credentials.
pub async fn watch(app: Arc<App>) {
    let read = |key: &str| dc_core::registry::read_env(&app.dir, key);
    let (Some(id), Some(secret)) = (read("TWITCH_CLIENT_ID"), read("TWITCH_CLIENT_SECRET")) else {
        tracing::info!("no TWITCH_CLIENT_ID / TWITCH_CLIENT_SECRET: Twitch alerts are off");
        return;
    };
    let mut client = Client {
        id,
        secret,
        token: None,
    };
    let mut was_live: HashSet<String> = HashSet::new();
    let mut first = true;
    loop {
        tokio::time::sleep(POLL).await;
        // Who follows which streamer, on the servers the bot is in.
        let guilds: Vec<String> = app.guilds.lock().unwrap().keys().cloned().collect();
        let mut followers: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
        for guild in guilds {
            let config = app.config(&guild);
            let twitch = &config.twitch;
            let Some(channel) = twitch.channel.clone().filter(|_| twitch.enabled) else {
                continue;
            };
            for login in &twitch.logins {
                followers
                    .entry(login.clone())
                    .or_default()
                    .push((channel.clone(), twitch.role.clone()));
            }
        }
        if followers.is_empty() {
            continue;
        }
        let logins: Vec<String> = followers.keys().cloned().collect();
        let mut live = HashMap::new();
        let mut failed = false;
        for chunk in logins.chunks(100) {
            match client.live(&app, chunk).await {
                Ok(found) => live.extend(found),
                Err(error) => {
                    tracing::warn!("checking Twitch failed: {error:#}");
                    failed = true;
                }
            }
        }
        if failed {
            continue;
        }
        for (login, stream) in &live {
            // Streams already live when the bot started are not announced.
            if was_live.contains(login) || first {
                continue;
            }
            for (channel, role) in followers.get(login).into_iter().flatten() {
                let msg = alert(login, stream, role.as_deref());
                if let Err(error) = app.api.send_message(channel, &msg).await {
                    tracing::warn!(login, %error, "posting a Twitch alert failed");
                }
            }
        }
        was_live = live.into_keys().collect();
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn alerts_ping_only_their_role() {
        let stream = json!({ "user_name": "Junko", "title": "Trading live", "thumbnail_url": "https://x/{width}x{height}.jpg", "game_name": "Crypto" });
        let payload = alert("junkocry", &stream, Some("42")).payload();
        let text = payload.to_string();
        assert!(text.contains("Junko is live") && text.contains("640x360"));
        assert_eq!(payload["allowed_mentions"]["roles"][0], "42");
        assert!(text.contains("https://twitch.tv/junkocry"));
        let quiet = alert("junkocry", &stream, None).payload();
        assert_eq!(quiet["allowed_mentions"]["roles"], json!([]));
    }
}
