//! Entry point for slash commands, buttons, menus and modals.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dc_core::guild::GuildConfig;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{
    api::ApiResult,
    app::{App, bits_of, roles_of, str_of},
    giveaway, moderation, settings, tickets,
    ui::Msg,
    verify, xp,
};

const PERM_ADMINISTRATOR: u64 = 1 << 3;

/// One interaction and everything needed to answer it.
pub struct Ctx {
    pub app: Arc<App>,
    id: String,
    token: String,
    pub guild_id: String,
    pub channel_id: String,
    pub user: Value,
    pub member: Value,
    /// Command name and options, or the component's `custom_id` and values.
    pub data: Value,
    deferred: AtomicBool,
}

impl Ctx {
    pub fn user_id(&self) -> &str {
        str_of(&self.user, "id")
    }

    pub fn config(&self) -> Arc<GuildConfig> {
        self.app.config(&self.guild_id)
    }

    /// True for bot admins; Discord computes the member's permissions for us.
    pub fn is_admin(&self) -> bool {
        bits_of(&self.member["permissions"]) & PERM_ADMINISTRATOR != 0
            || self
                .app
                .is_admin(&self.guild_id, self.user_id(), &roles_of(&self.member))
    }

    async fn callback(&self, body: Value, files: &[(String, Vec<u8>)]) {
        let path = format!("/interactions/{}/{}/callback", self.id, self.token);
        if let Err(error) = self
            .app
            .api
            .send(Method::POST, &path, Some(&body), None, files)
            .await
        {
            tracing::warn!(%error, "answering an interaction failed");
        }
    }

    /// Tells Discord the answer is coming (required when it takes over 3 s).
    pub async fn defer(&self, ephemeral: bool) {
        let flags = if ephemeral { 1 << 6 } else { 0 };
        self.callback(json!({ "type": 5, "data": { "flags": flags } }), &[])
            .await;
        self.deferred.store(true, Ordering::Relaxed);
    }

    /// For a button or menu: the answer will replace the message it belongs
    /// to (`reply` then edits that message).
    pub async fn defer_update(&self) {
        self.callback(json!({ "type": 6 }), &[]).await;
        self.deferred.store(true, Ordering::Relaxed);
    }

    /// An extra message after the answer, visible to everyone.
    pub async fn followup(&self, msg: Msg) {
        let msg = msg.finish().await;
        let path = format!("/webhooks/{}/{}", self.app.app_id(), self.token);
        if let Err(error) = self
            .app
            .api
            .send(Method::POST, &path, Some(&msg.payload()), None, &msg.files)
            .await
        {
            tracing::warn!(%error, "sending a follow-up message failed");
        }
    }

    /// `custom_id` of the clicked button or menu.
    pub fn custom_id(&self) -> &str {
        str_of(&self.data, "custom_id")
    }

    /// Values picked in a menu.
    pub fn values(&self) -> Vec<String> {
        self.data["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect()
    }

    /// Answers the interaction, or fills in the deferred answer.
    pub async fn reply(&self, msg: Msg) {
        let msg = msg.finish().await;
        if self.deferred.load(Ordering::Relaxed) {
            // Whether the answer is private was decided when deferring.
            let mut msg = msg;
            msg.ephemeral = false;
            let path = format!(
                "/webhooks/{}/{}/messages/@original",
                self.app.app_id(),
                self.token
            );
            if let Err(error) = self
                .app
                .api
                .send(Method::PATCH, &path, Some(&msg.payload()), None, &msg.files)
                .await
            {
                tracing::warn!(%error, "editing a deferred answer failed");
            }
        } else {
            self.callback(json!({ "type": 4, "data": msg.payload() }), &msg.files)
                .await;
        }
    }

    /// Replaces the message the clicked button belongs to.
    pub async fn update(&self, msg: Msg) {
        let mut msg = msg.finish().await;
        msg.ephemeral = false;
        self.callback(json!({ "type": 7, "data": msg.payload() }), &msg.files)
            .await;
    }

    /// Opens the application's Activity for the member (callback type 12).
    /// On failure nothing was sent, so the interaction can still be answered.
    pub async fn launch_activity(&self) -> ApiResult<()> {
        let path = format!("/interactions/{}/{}/callback", self.id, self.token);
        self.app.api.post(&path, &json!({ "type": 12 })).await?;
        Ok(())
    }

    pub async fn modal(&self, modal: Value) {
        self.callback(json!({ "type": 9, "data": modal }), &[])
            .await;
    }
}

/// Options of a slash command, with the subcommand unwrapped.
pub struct Options {
    pub sub: Option<String>,
    values: HashMap<String, Value>,
    resolved: Value,
}

impl Options {
    fn parse(data: &Value) -> Self {
        let mut sub = None;
        let mut list = data["options"].as_array().cloned().unwrap_or_default();
        // Type 1 is a subcommand: its own options are one level down.
        if let Some(first) = list.first()
            && first["type"] == 1
        {
            sub = first["name"].as_str().map(str::to_owned);
            list = first["options"].as_array().cloned().unwrap_or_default();
        }
        let values = list
            .into_iter()
            .filter_map(|option| {
                Some((option["name"].as_str()?.to_owned(), option["value"].clone()))
            })
            .collect();
        Self {
            sub,
            values,
            resolved: data["resolved"].clone(),
        }
    }

    /// String options, and the id of user / channel / role options.
    pub fn str(&self, name: &str) -> Option<&str> {
        self.values.get(name)?.as_str()
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.values.get(name)?.as_i64()
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.values.get(name)?.as_bool()
    }

    /// The user object of a user option.
    pub fn user(&self, name: &str) -> Option<&Value> {
        let user = &self.resolved["users"][self.str(name)?];
        user.is_object().then_some(user)
    }

    /// The server member of a user option; null when they are not on the server.
    pub fn member(&self, name: &str) -> &Value {
        match self.str(name) {
            Some(id) => &self.resolved["members"][id],
            None => &Value::Null,
        }
    }
}

pub async fn handle(app: Arc<App>, interaction: Value) {
    let kind = interaction["type"].as_u64().unwrap_or(0);
    let member = interaction["member"].clone();
    let ctx = Ctx {
        app,
        id: str_of(&interaction, "id").to_owned(),
        token: str_of(&interaction, "token").to_owned(),
        guild_id: str_of(&interaction, "guild_id").to_owned(),
        channel_id: str_of(&interaction["channel"], "id").to_owned(),
        user: member["user"].clone(),
        member,
        data: interaction["data"].clone(),
        deferred: AtomicBool::new(false),
    };
    if ctx.guild_id.is_empty() {
        ctx.reply(Msg::error("This bot only works inside a server."))
            .await;
        return;
    }

    match kind {
        2 => command(&ctx).await,
        3 => component(&ctx).await,
        5 => modal_submit(&ctx).await,
        _ => {}
    }
}

async fn command(ctx: &Ctx) {
    let options = Options::parse(&ctx.data);
    let name = str_of(&ctx.data, "name");

    // Commands open to every member.
    match name {
        "rank" => return xp::rank(ctx, &options).await,
        "leaderboard" => return xp::leaderboard(ctx).await,
        "help" => return settings::help(ctx).await,
        _ => {}
    }

    if !ctx.is_admin() {
        return ctx.reply(Msg::denied()).await;
    }
    match name {
        "lock" => moderation::set_channel_lock(ctx, &options, moderation::Access::Lock).await,
        "unlock" => moderation::set_channel_lock(ctx, &options, moderation::Access::Unlock).await,
        "hide" => moderation::set_channel_lock(ctx, &options, moderation::Access::Hide).await,
        "unhide" => moderation::set_channel_lock(ctx, &options, moderation::Access::Unhide).await,
        "lockurl" => moderation::lock_url(ctx, &options).await,
        "ownerbot" => moderation::owner_bot(ctx, &options).await,
        "mute" => moderation::mute(ctx, &options).await,
        "unmute" => moderation::unmute(ctx, &options).await,
        "ban" => moderation::ban(ctx, &options).await,
        "unban" => moderation::unban(ctx, &options).await,
        "kick" => moderation::kick(ctx, &options).await,
        "role" => moderation::role(ctx, &options).await,
        "xp" => xp::manage(ctx, &options).await,
        "giveaway" => giveaway::command(ctx, &options).await,
        "settings" => settings::command(ctx, &options).await,
        _ => ctx.reply(Msg::error("Unknown command.")).await,
    }
}

async fn component(ctx: &Ctx) {
    let id = ctx.custom_id();
    if id.starts_with("settings:") {
        return settings::component(ctx).await;
    }
    if id.starts_with("confirm:") {
        return moderation::confirmed(ctx).await;
    }
    if id.starts_with("giveaway:") {
        return giveaway::component(ctx).await;
    }
    match id {
        "verify:start" => verify::start(ctx).await,
        "verify:enter" => verify::ask_code(ctx).await,
        "ticket:open" | "ticket:topic" => tickets::open(ctx).await,
        "ticket:claim" => tickets::claim(ctx).await,
        "ticket:close" => tickets::ask_close(ctx).await,
        "ticket:close:yes" => tickets::close(ctx).await,
        "ticket:close:no" => tickets::cancel_close(ctx).await,
        _ => {
            ctx.reply(Msg::error("This button is no longer active."))
                .await
        }
    }
}

async fn modal_submit(ctx: &Ctx) {
    match str_of(&ctx.data, "custom_id") {
        "verify:code" => verify::check_code(ctx).await,
        _ => {
            ctx.reply(Msg::error("This form is no longer active."))
                .await
        }
    }
}

/// Value typed in a modal's text input.
pub fn modal_value<'a>(data: &'a Value, input_id: &str) -> &'a str {
    data["components"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["components"].as_array().into_iter().flatten())
        .find(|input| input["custom_id"] == input_id)
        .and_then(|input| input["value"].as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_unwrap_subcommands() {
        let data = json!({
            "name": "ownerbot",
            "options": [{ "type": 1, "name": "add", "options": [
                { "type": 6, "name": "user", "value": "42" },
                { "type": 5, "name": "flag", "value": true },
                { "type": 4, "name": "days", "value": 7 }
            ]}],
            "resolved": { "users": { "42": { "id": "42", "username": "bob" } } }
        });
        let options = Options::parse(&data);
        assert_eq!(options.sub.as_deref(), Some("add"));
        assert_eq!(options.str("user"), Some("42"));
        assert_eq!(options.bool("flag"), Some(true));
        assert_eq!(options.int("days"), Some(7));
        assert_eq!(options.user("user").unwrap()["username"], "bob");
        assert!(options.member("user").is_null());
        assert!(options.user("missing").is_none());
    }

    #[test]
    fn options_without_subcommand() {
        let data = json!({ "name": "kick", "options": [{ "type": 3, "name": "reason", "value": "spam" }] });
        let options = Options::parse(&data);
        assert_eq!(options.sub, None);
        assert_eq!(options.str("reason"), Some("spam"));
    }

    #[test]
    fn modal_values() {
        let data = json!({ "components": [{ "type": 1, "components": [
            { "type": 4, "custom_id": "code", "value": "AB12C" }
        ]}]});
        assert_eq!(modal_value(&data, "code"), "AB12C");
        assert_eq!(modal_value(&data, "other"), "");
    }
}
