//! Moderation commands: channel locks, link blocking, bot admins, mute,
//! ban, kick and roles.

use std::time::{Duration, Instant};

use dc_core::{cards::Tone, crypto::random_token, guild::normalize_domain, is_snowflake};
use serde_json::{Value, json};

use crate::{
    app::{App, bits_of, display_name, format_duration, iso8601, parse_duration, roles_of, str_of},
    interactions::{Ctx, Options},
    logs, massrole,
    ui::{self, Msg},
};

const VIEW_CHANNEL: u64 = 1 << 10;
const SEND_MESSAGES: u64 = 1 << 11;
const SEND_IN_THREADS: u64 = 1 << 38;
const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
const CREATE_PRIVATE_THREADS: u64 = 1 << 36;
const WRITE: u64 = SEND_MESSAGES | SEND_IN_THREADS | CREATE_PUBLIC_THREADS | CREATE_PRIVATE_THREADS;

/// Discord's maximum timeout.
const MAX_MUTE_SECS: i64 = 28 * 86_400;
const NOTICE_LIFETIME: Duration = Duration::from_secs(6);
/// How long a ban or kick waits for the moderator's confirmation.
const CONFIRM_LIFETIME: Duration = Duration::from_secs(120);

#[derive(Clone, Copy)]
pub enum Access {
    Lock,
    Unlock,
    Hide,
    Unhide,
}

/// `/lock`, `/unlock`, `/hide`, `/unhide`: edits the @everyone permissions of
/// a channel, leaving every other permission untouched.
pub async fn set_channel_lock(ctx: &Ctx, options: &Options, action: Access) {
    let channel_id = options.str("channel").unwrap_or(&ctx.channel_id).to_owned();
    let api = &ctx.app.api;
    ctx.defer(false).await;

    let channel = match api.get(&format!("/channels/{channel_id}")).await {
        Ok(channel) => channel,
        Err(error) => return ctx.reply(Msg::error(&error.user_message())).await,
    };
    // The @everyone overwrite has the id of the server.
    let everyone = channel["permission_overwrites"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|overwrite| overwrite["id"] == ctx.guild_id.as_str());
    let (allow, deny) = match everyone {
        Some(overwrite) => (bits_of(&overwrite["allow"]), bits_of(&overwrite["deny"])),
        None => (0, 0),
    };

    let (bits, restrict, done, already, verb) = match action {
        Access::Lock => (WRITE, true, "Channel locked", "is already locked", "locked"),
        Access::Unlock => (
            WRITE,
            false,
            "Channel unlocked",
            "is not locked",
            "unlocked",
        ),
        Access::Hide => (
            VIEW_CHANNEL,
            true,
            "Channel hidden",
            "is already hidden",
            "hidden",
        ),
        Access::Unhide => (
            VIEW_CHANNEL,
            false,
            "Channel visible",
            "is not hidden",
            "made visible",
        ),
    };
    let is_restricted = deny & bits == bits;
    if is_restricted == restrict {
        let text = format!("<#{channel_id}> {already}.");
        return ctx.reply(Msg::warning("Nothing to do", &text)).await;
    }

    let reason = format!("{verb} by {}", str_of(&ctx.user, "username"));
    if restrict {
        // Keep the bot able to read and answer in the channel it restricts.
        let own = format!("/channels/{channel_id}/permissions/{}", ctx.app.user_id());
        let allow_self = (VIEW_CHANNEL | SEND_MESSAGES).to_string();
        let body = json!({ "type": 1, "allow": allow_self, "deny": "0" });
        if let Err(error) = api.put(&own, &body, Some(&reason)).await {
            return ctx.reply(Msg::error(&error.user_message())).await;
        }
    }
    let (allow, deny) = if restrict {
        (allow & !bits, deny | bits)
    } else {
        (allow, deny & !bits)
    };
    let path = format!("/channels/{channel_id}/permissions/{}", ctx.guild_id);
    let body = json!({ "type": 0, "allow": allow.to_string(), "deny": deny.to_string() });
    if let Err(error) = api.put(&path, &body, Some(&reason)).await {
        return ctx.reply(Msg::error(&error.user_message())).await;
    }

    let mut text = format!("<#{channel_id}> was {verb} by <@{}>.", ctx.user_id());
    if let Some(reason) = options.str("reason") {
        text.push_str(&format!("\n**Reason:** {}", ui::escape(reason)));
    }
    let color = if restrict { ui::AMBER } else { ui::GREEN };
    ctx.reply(Msg::notice(color, done, &text)).await;
    logs::moderation(&ctx.app, &ctx.guild_id, done, &text).await;
}

/// `/lockurl`: turns the link filter on or off.
pub async fn lock_url(ctx: &Ctx, options: &Options) {
    let enabled = options.bool("enabled").unwrap_or(true);
    if let Err(error) = ctx
        .app
        .update_config(&ctx.guild_id, |config| config.lock_url.enabled = enabled)
    {
        tracing::error!(%error, "saving the configuration failed");
        return ctx
            .reply(Msg::error("The setting could not be saved."))
            .await;
    }
    let (title, text) = if enabled {
        (
            "Links locked",
            "Links posted by members are now deleted. Bot admins and exempt roles or channels are not affected.",
        )
    } else {
        ("Links unlocked", "Members can post links again.")
    };
    ctx.reply(Msg::success(title, text)).await;
    let entry = format!("{title} by <@{}>.", ctx.user_id());
    logs::moderation(&ctx.app, &ctx.guild_id, title, &entry).await;
}

/// `/ownerbot`: manages the bot admins of the server. Reserved to the server
/// owner and to the bot's owners on the dashboard.
pub async fn owner_bot(ctx: &Ctx, options: &Options) {
    let sub = options.sub.as_deref().unwrap_or("list");
    if sub == "list" {
        let config = ctx.config();
        let guild = ctx.app.guild(&ctx.guild_id);
        let mut lines = vec![format!("<@{}> · server owner", guild.owner_id)];
        lines.extend(
            ctx.app
                .owners
                .iter()
                .map(|id| format!("<@{id}> · bot owner")),
        );
        lines.extend(config.admins.users.iter().map(|id| format!("<@{id}>")));
        lines.extend(
            config
                .admins
                .roles
                .iter()
                .map(|id| format!("<@&{id}> · role")),
        );
        let text = format!(
            "{}\n-# Members with Discord's Administrator permission are admins too.",
            lines.join("\n")
        );
        return ctx
            .reply(Msg::notice(ui::WHITE, "Bot admins", &text).ephemeral())
            .await;
    }

    if !ctx.app.is_super_admin(&ctx.guild_id, ctx.user_id()) {
        let text = "Only the server owner and the bot's owners can change the admin list.";
        return ctx
            .reply(Msg::notice(ui::RED, "Not allowed", text).ephemeral())
            .await;
    }
    let Some(user) = options.user("user") else {
        return ctx.reply(Msg::error("Unknown user.")).await;
    };
    let target = str_of(user, "id").to_owned();
    if user["bot"] == true {
        return ctx.reply(Msg::error("A bot cannot be a bot admin.")).await;
    }

    let add = sub == "add";
    let mut changed = false;
    let saved = ctx.app.update_config(&ctx.guild_id, |config| {
        let users = &mut config.admins.users;
        let present = users.contains(&target);
        if add && !present {
            users.push(target.clone());
            changed = true;
        } else if !add && present {
            users.retain(|id| id != &target);
            changed = true;
        }
    });
    if let Err(error) = saved {
        tracing::error!(%error, "saving the configuration failed");
        return ctx
            .reply(Msg::error("The admin list could not be saved."))
            .await;
    }

    let msg = match (add, changed) {
        (true, true) => Msg::success(
            "Admin added",
            &format!("<@{target}> can now use the bot's admin commands."),
        ),
        (true, false) => Msg::warning(
            "Nothing to do",
            &format!("<@{target}> is already a bot admin."),
        ),
        (false, true) => Msg::success(
            "Admin removed",
            &format!("<@{target}> is no longer a bot admin."),
        ),
        (false, false) => Msg::warning(
            "Nothing to do",
            &format!("<@{target}> was not in the admin list."),
        ),
    };
    ctx.reply(msg.ephemeral()).await;
    if changed {
        let title = if add {
            "Bot admin added"
        } else {
            "Bot admin removed"
        };
        let entry = format!("<@{target}> · by <@{}>", ctx.user_id());
        logs::moderation(&ctx.app, &ctx.guild_id, title, &entry).await;
    }
}

/// Refuses sanctions that make no sense or would let admins fight each other.
fn check_target(ctx: &Ctx, target_id: &str, target_member: &Value) -> Result<(), &'static str> {
    let app = &ctx.app;
    if target_id == ctx.user_id() {
        return Err("You cannot do that to yourself.");
    }
    if target_id == app.user_id() {
        return Err("I cannot do that to myself.");
    }
    if app.guild(&ctx.guild_id).owner_id == target_id {
        return Err("The server owner cannot be sanctioned.");
    }
    let target_is_admin = app.is_admin(&ctx.guild_id, target_id, &roles_of(target_member));
    if target_is_admin && !app.is_super_admin(&ctx.guild_id, ctx.user_id()) {
        return Err("Only the server owner or a bot owner can sanction another bot admin.");
    }
    // Like Discord itself: nobody acts on someone placed at or above them.
    if !target_member.is_null() && !ctx.is_trusted() {
        let mine = app.top_position(&ctx.guild_id, &roles_of(&ctx.member));
        let theirs = app.top_position(&ctx.guild_id, &roles_of(target_member));
        if theirs >= mine {
            return Err("That member's highest role is at or above yours.");
        }
    }
    Ok(())
}

/// May this member give or take this role with the bot? Server admins and
/// bot owners may; other bot admins only roles below their own highest role
/// and without moderation permissions, so the bot never lets anyone grant
/// themselves more power than they have.
pub fn may_manage_role(ctx: &Ctx, role: &str) -> Result<(), &'static str> {
    if role == ctx.guild_id {
        return Err("@everyone cannot be given or taken.");
    }
    if ctx.is_trusted() {
        return Ok(());
    }
    let app = &ctx.app;
    let permissions = app
        .guild(&ctx.guild_id)
        .roles
        .get(role)
        .copied()
        .unwrap_or(0);
    if permissions & POWERFUL != 0 {
        return Err(
            "This role has moderation or admin permissions. Only the server owner, \
             Discord administrators and the bot's owners can hand it out.",
        );
    }
    let mine = app.top_position(&ctx.guild_id, &roles_of(&ctx.member));
    let role_position = app.top_position(&ctx.guild_id, &[role.to_owned()]);
    if role_position >= mine {
        return Err("You can only hand out roles below your highest role.");
    }
    Ok(())
}

fn sanction_text(target: &str, detail: &str, reason: &str, by: &str) -> String {
    format!(
        "**Member:** <@{target}>{detail}\n**Reason:** {}\n**By:** <@{by}>",
        ui::escape(reason)
    )
}

/// Shared flow of the sanction commands: checks, the Discord call, the
/// public confirmation and the log entry.
async fn sanction(
    ctx: &Ctx,
    options: &Options,
    title: &str,
    detail: &str,
    must_be_member: bool,
    call: impl AsyncFnOnce(&App, &str, &str) -> crate::api::ApiResult,
) {
    let Some(target) = options.str("user").map(str::to_owned) else {
        return ctx.reply(Msg::error("Unknown user.")).await;
    };
    let member = options.member("user");
    if must_be_member && member.is_null() {
        return ctx
            .reply(Msg::error("That user is not on this server."))
            .await;
    }
    if let Err(message) = check_target(ctx, &target, member) {
        return ctx.reply(Msg::error(message)).await;
    }

    ctx.defer(false).await;
    let reason = options.str("reason").unwrap_or("No reason given");
    let audit = format!("{} — by {}", reason, str_of(&ctx.user, "username"));
    if let Err(error) = call(&ctx.app, &target, &audit).await {
        return ctx.reply(Msg::error(&error.user_message())).await;
    }

    let text = sanction_text(&target, detail, reason, ctx.user_id());
    ctx.reply(Msg::success(title, &text)).await;
    logs::moderation(&ctx.app, &ctx.guild_id, title, &text).await;
}

pub async fn mute(ctx: &Ctx, options: &Options) {
    let Some(secs) = options.str("duration").and_then(parse_duration) else {
        let text = "Use a duration such as `10m`, `2h` or `1d`.";
        return ctx.reply(Msg::error(text)).await;
    };
    if secs > MAX_MUTE_SECS {
        return ctx
            .reply(Msg::error("Discord limits timeouts to 28 days."))
            .await;
    }
    let until = dc_core::now() + secs;
    let detail = format!(
        "\n**Duration:** {} (until <t:{until}:f>)",
        format_duration(secs)
    );
    let guild = ctx.guild_id.clone();
    sanction(
        ctx,
        options,
        "Member muted",
        &detail,
        true,
        async move |app, user, reason| {
            let body = json!({ "communication_disabled_until": iso8601(until) });
            app.api
                .patch(
                    &format!("/guilds/{guild}/members/{user}"),
                    &body,
                    Some(reason),
                )
                .await
        },
    )
    .await;
}

pub async fn unmute(ctx: &Ctx, options: &Options) {
    let guild = ctx.guild_id.clone();
    sanction(
        ctx,
        options,
        "Member unmuted",
        "",
        true,
        async move |app, user, reason| {
            let body = json!({ "communication_disabled_until": null });
            app.api
                .patch(
                    &format!("/guilds/{guild}/members/{user}"),
                    &body,
                    Some(reason),
                )
                .await
        },
    )
    .await;
}

/// An action waiting for the moderator's confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmable {
    Ban {
        delete_days: i64,
    },
    Kick,
    /// A role (the pending target) given to or taken from every member.
    MassRole {
        add: bool,
        include_bots: bool,
    },
}

impl Confirmable {
    fn verb(self) -> &'static str {
        match self {
            Self::Ban { .. } => "Ban",
            Self::Kick => "Kick",
            Self::MassRole { add: true, .. } => "Give to everyone",
            Self::MassRole { add: false, .. } => "Take from everyone",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Self::Ban { .. } => "Member banned",
            Self::Kick => "Member kicked",
            Self::MassRole { .. } => "Started",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pending {
    removal: Confirmable,
    guild_id: String,
    target: String,
    reason: String,
    /// Only the moderator who asked can confirm.
    author: String,
    expires: Instant,
}

pub async fn ban(ctx: &Ctx, options: &Options) {
    let delete_days = options.int("delete_days").unwrap_or(0).clamp(0, 7);
    ask_removal(ctx, options, Confirmable::Ban { delete_days }).await;
}

pub async fn kick(ctx: &Ctx, options: &Options) {
    ask_removal(ctx, options, Confirmable::Kick).await;
}

/// Keeps an action until it is confirmed; returns its id for the buttons.
fn remember(ctx: &Ctx, removal: Confirmable, target: &str, reason: &str) -> String {
    let id = random_token(8);
    let mut pending = ctx.app.confirmations.lock().unwrap();
    pending.retain(|_, p| p.expires > Instant::now());
    pending.insert(
        id.clone(),
        Pending {
            removal,
            guild_id: ctx.guild_id.clone(),
            target: target.to_owned(),
            reason: reason.to_owned(),
            author: ctx.user_id().to_owned(),
            expires: Instant::now() + CONFIRM_LIFETIME,
        },
    );
    id
}

fn confirm_buttons(removal: Confirmable, id: &str) -> Value {
    ui::row(vec![
        ui::button(
            ui::STYLE_DANGER,
            removal.verb(),
            &format!("confirm:yes:{id}"),
        ),
        ui::button(ui::STYLE_SECONDARY, "Cancel", &format!("confirm:no:{id}")),
    ])
}

/// `/ban` and `/kick` first show what will happen, with Confirm / Cancel.
async fn ask_removal(ctx: &Ctx, options: &Options, removal: Confirmable) {
    let (Some(target), Some(user)) = (options.str("user"), options.user("user")) else {
        return ctx.reply(Msg::error("Unknown user.")).await;
    };
    let member = options.member("user");
    if removal == Confirmable::Kick && member.is_null() {
        return ctx
            .reply(Msg::error("That user is not on this server."))
            .await;
    }
    if let Err(message) = check_target(ctx, target, member) {
        return ctx.reply(Msg::error(message)).await;
    }
    let reason = options
        .str("reason")
        .unwrap_or("No reason given")
        .to_owned();
    let id = remember(ctx, removal, target, &reason);

    let name = display_name(user, member);
    let consequence = match removal {
        Confirmable::Ban { delete_days: 0 } => {
            "They are removed from the server and cannot join again until unbanned.".to_owned()
        }
        Confirmable::Ban { delete_days } => format!(
            "They are removed from the server, cannot join again until unbanned, and their \
             messages of the last {delete_days} day(s) are deleted."
        ),
        Confirmable::Kick => {
            "They are removed from the server but can join again with an invite.".into()
        }
        Confirmable::MassRole { .. } => String::new(),
    };
    let details = format!(
        "**Member:** <@{target}>\n**Reason:** {}",
        ui::escape(&reason)
    );
    let buttons = confirm_buttons(removal, &id);
    let msg = Msg::banner(
        Tone::Confirm,
        &format!("{} {name}?", removal.verb()),
        &consequence,
        vec![ui::text(details), ui::separator(), buttons],
    );
    ctx.reply(msg.ephemeral()).await;
}

/// Click on Confirm or Cancel under a ban or kick.
pub async fn confirmed(ctx: &Ctx) {
    let mut parts = ctx.custom_id().splitn(3, ':').skip(1);
    let (answer, id) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let pending = ctx.app.confirmations.lock().unwrap().get(id).cloned();
    let Some(pending) = pending.filter(|p| p.expires > Instant::now()) else {
        let msg = Msg::banner(
            Tone::Warning,
            "Expired",
            "Run the command again.",
            Vec::new(),
        );
        return ctx.update(msg).await;
    };
    if pending.author != ctx.user_id() {
        return ctx
            .reply(Msg::denied_with(
                "Only the moderator who ran the command can confirm it.",
            ))
            .await;
    }
    ctx.app.confirmations.lock().unwrap().remove(id);
    if answer != "yes" {
        let msg = Msg::banner(Tone::Info, "Cancelled", "Nothing was done.", Vec::new());
        return ctx.update(msg).await;
    }
    if !ctx.is_admin() || pending.guild_id != ctx.guild_id {
        return ctx.update(Msg::denied()).await;
    }

    if let Confirmable::MassRole { add, include_bots } = pending.removal {
        return start_mass_role(ctx, &pending, add, include_bots).await;
    }

    ctx.defer_update().await;
    let audit = format!("{} — by {}", pending.reason, str_of(&ctx.user, "username"));
    let (guild, target) = (&pending.guild_id, &pending.target);
    let result = match pending.removal {
        Confirmable::Ban { delete_days } => {
            let body = json!({ "delete_message_seconds": delete_days * 86_400 });
            let path = format!("/guilds/{guild}/bans/{target}");
            ctx.app.api.put(&path, &body, Some(&audit)).await
        }
        Confirmable::Kick => {
            let path = format!("/guilds/{guild}/members/{target}");
            ctx.app.api.delete(&path, Some(&audit)).await
        }
        Confirmable::MassRole { .. } => unreachable!("handled above"),
    };
    if let Err(error) = result {
        return ctx.reply(Msg::error(&error.user_message())).await;
    }

    let title = pending.removal.done();
    let text = sanction_text(target, "", &pending.reason, ctx.user_id());
    ctx.reply(Msg::banner(Tone::Success, "Done", title, Vec::new()))
        .await;
    // The result is public, like the other moderation commands.
    ctx.followup(Msg::success(title, &text)).await;
    logs::moderation(&ctx.app, guild, title, &text).await;
}

pub async fn unban(ctx: &Ctx, options: &Options) {
    let target = options.str("user_id").unwrap_or("").trim().to_owned();
    if !is_snowflake(&target) {
        return ctx.reply(Msg::error("That is not a valid user id.")).await;
    }
    ctx.defer(false).await;
    let reason = format!("Unbanned by {}", str_of(&ctx.user, "username"));
    let path = format!("/guilds/{}/bans/{target}", ctx.guild_id);
    match ctx.app.api.delete(&path, Some(&reason)).await {
        Ok(_) => {
            let text = format!("**User:** <@{target}>\n**By:** <@{}>", ctx.user_id());
            ctx.reply(Msg::notice(ui::WHITE, "Ban lifted", &text)).await;
            logs::moderation(&ctx.app, &ctx.guild_id, "Ban lifted", &text).await;
        }
        Err(error) if error.is_not_found() => {
            ctx.reply(Msg::error("That user is not banned.")).await;
        }
        Err(error) => ctx.reply(Msg::error(&error.user_message())).await,
    }
}

/// Permissions that make a role dangerous to hand out to everyone.
const POWERFUL: u64 = (1 << 3) // administrator
    | (1 << 1) // kick
    | (1 << 2) // ban
    | (1 << 4) // manage channels
    | (1 << 5) // manage server
    | (1 << 13) // manage messages
    | (1 << 28) // manage roles
    | (1 << 29) // manage webhooks
    | (1 << 40); // moderate members

/// `/role add-all` and `/role remove-all`: asks for confirmation first.
async fn ask_mass_role(ctx: &Ctx, options: &Options, add: bool) {
    let Some(role) = options.str("role").map(str::to_owned) else {
        return ctx.reply(Msg::error("Pick a role.")).await;
    };
    if role == ctx.guild_id {
        return ctx
            .reply(Msg::error("Everyone already has @everyone."))
            .await;
    }
    let guild = ctx.app.guild(&ctx.guild_id);
    let permissions = guild.roles.get(&role).copied().unwrap_or(0);
    // Even for trusted members: handing admin rights to the whole server is
    // never what anyone wants.
    if permissions & POWERFUL != 0 && add {
        return ctx
            .reply(Msg::denied_with(
                "This role has moderation or admin permissions: it cannot be given to everyone.",
            ))
            .await;
    }
    if let Err(reason) = may_manage_role(ctx, &role) {
        return ctx.reply(Msg::denied_with(reason)).await;
    }
    if ctx.app.mass_roles.lock().unwrap().contains(&ctx.guild_id) {
        let text = "A role is already being given or taken on this server. Wait until it ends.";
        return ctx.reply(Msg::warning("Busy", text).ephemeral()).await;
    }

    let include_bots = options.bool("include_bots").unwrap_or(false);
    let removal = Confirmable::MassRole { add, include_bots };
    let id = remember(ctx, removal, &role, "");
    let role_name = ctx.app.role_name(&ctx.guild_id, &role);
    let who = if include_bots {
        "members and bots"
    } else {
        "members (bots excluded)"
    };
    let (title, consequence) = if add {
        (
            format!("Give {role_name} to everyone?"),
            format!(
                "All {who} of the server get this role. With many members it can take a while."
            ),
        )
    } else {
        (
            format!("Take {role_name} from everyone?"),
            format!(
                "All {who} of the server lose this role. With many members it can take a while."
            ),
        )
    };
    let details = format!(
        "**Role:** <@&{role}>\n**Members on the server:** {}",
        ui::number(guild.member_count)
    );
    let msg = Msg::banner(
        Tone::Confirm,
        &title,
        &consequence,
        vec![
            ui::text(details),
            ui::separator(),
            confirm_buttons(removal, &id),
        ],
    );
    ctx.reply(msg.ephemeral()).await;
}

async fn start_mass_role(ctx: &Ctx, pending: &Pending, add: bool, include_bots: bool) {
    // One job per server: reserved here, released when the job ends.
    if !ctx
        .app
        .mass_roles
        .lock()
        .unwrap()
        .insert(ctx.guild_id.clone())
    {
        let text = "A role is already being given or taken on this server.";
        return ctx.update(Msg::warning("Busy", text)).await;
    }
    let job = massrole::Job {
        guild_id: ctx.guild_id.clone(),
        role_id: pending.target.clone(),
        add,
        include_bots,
        channel_id: ctx.channel_id.clone(),
        by: ctx.user_id().to_owned(),
    };
    let text = "Follow the progress in this channel. I will post the result when it is done.";
    ctx.update(Msg::banner(Tone::Success, "Started", text, Vec::new()))
        .await;
    tokio::spawn(massrole::run(ctx.app.clone(), job));
}

/// `/role add`, `/role remove`, `/role add-all` and `/role remove-all`.
pub async fn role(ctx: &Ctx, options: &Options) {
    match options.sub.as_deref() {
        Some("add-all") => return ask_mass_role(ctx, options, true).await,
        Some("remove-all") => return ask_mass_role(ctx, options, false).await,
        _ => {}
    }
    let (Some(target), Some(role)) = (options.str("user"), options.str("role")) else {
        return ctx.reply(Msg::error("Pick a member and a role.")).await;
    };
    if options.member("user").is_null() {
        return ctx
            .reply(Msg::error("That user is not on this server."))
            .await;
    }
    if let Err(reason) = may_manage_role(ctx, role) {
        return ctx.reply(Msg::denied_with(reason)).await;
    }
    ctx.defer(false).await;
    let add = options.sub.as_deref() != Some("remove");
    let reason = format!("By {}", str_of(&ctx.user, "username"));
    let api = &ctx.app.api;
    let result = if add {
        api.add_role(&ctx.guild_id, target, role, &reason).await
    } else {
        api.remove_role(&ctx.guild_id, target, role, &reason).await
    };
    if let Err(error) = result {
        return ctx.reply(Msg::error(&error.user_message())).await;
    }
    let (title, verb) = if add {
        ("Role added", "now has")
    } else {
        ("Role removed", "no longer has")
    };
    let text = format!(
        "<@{target}> {verb} <@&{role}>.\n**By:** <@{}>",
        ctx.user_id()
    );
    ctx.reply(Msg::success(title, &text)).await;
    logs::moderation(&ctx.app, &ctx.guild_id, title, &text).await;
}

// ---------------------------------------------------------------- link filter

/// First blocked link of a message, if any. Links to `allowed` domains (and
/// their subdomains) pass.
pub fn blocked_link(content: &str, allowed: &[String]) -> Option<String> {
    content
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '|' | '"')
        })
        .filter(|word| {
            let lower = word.to_lowercase();
            lower.contains("http://")
                || lower.contains("https://")
                || lower.starts_with("www.")
                || lower.contains("discord.gg/")
                || lower.contains("discord.com/invite/")
        })
        .find_map(|word| {
            // Markdown links glue text to the URL: start at the scheme.
            let start = word.to_lowercase().find("http").unwrap_or(0);
            let domain = normalize_domain(&word[start..]);
            let is_allowed = allowed
                .iter()
                .any(|ok| domain == *ok || domain.ends_with(&format!(".{ok}")));
            (!is_allowed).then_some(domain)
        })
}

/// Deletes the message if it has a link its author may not post. Returns
/// true when the message was removed.
pub async fn filter_links(app: &App, guild_id: &str, message: &Value) -> bool {
    let config = app.config(guild_id);
    let filter = &config.lock_url;
    let channel_id = str_of(message, "channel_id");
    if !filter.enabled || filter.exempt_channels.iter().any(|id| id == channel_id) {
        return false;
    }
    let Some(domain) = blocked_link(str_of(message, "content"), &filter.allowed_domains) else {
        return false;
    };
    let author = str_of(&message["author"], "id");
    let roles = roles_of(&message["member"]);
    if filter.exempt_roles.iter().any(|role| roles.contains(role))
        || app.is_admin(guild_id, author, &roles)
    {
        return false;
    }

    if let Err(error) = app
        .api
        .delete_message(channel_id, str_of(message, "id"))
        .await
    {
        tracing::warn!(guild = guild_id, %error, "deleting a link failed");
        return false;
    }
    let notice = Msg::notice(
        ui::AMBER,
        "Links are not allowed here",
        &format!("<@{author}>, your message was removed because it contained a link."),
    )
    .ping(author);
    // The notice is only useful for a few seconds: clean it up afterwards.
    if let Ok(sent) = app.api.send_message(channel_id, &notice).await {
        let (api, channel, id) = (
            app.api.clone(),
            channel_id.to_owned(),
            str_of(&sent, "id").to_owned(),
        );
        tokio::spawn(async move {
            tokio::time::sleep(NOTICE_LIFETIME).await;
            let _ = api.delete_message(&channel, &id).await;
        });
    }
    let entry =
        format!("**Author:** <@{author}>\n**Channel:** <#{channel_id}>\n**Domain:** `{domain}`");
    logs::moderation(app, guild_id, "Link removed", &entry).await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_detected() {
        let none: Vec<String> = Vec::new();
        assert_eq!(blocked_link("hello world", &none), None);
        assert_eq!(
            blocked_link("see https://evil.example/x?y", &none).as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            blocked_link("<HTTP://Evil.Example>", &none).as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            blocked_link("join discord.gg/abc", &none).as_deref(),
            Some("discord.gg")
        );
        assert_eq!(
            blocked_link("www.site.com now", &none).as_deref(),
            Some("site.com")
        );
        assert_eq!(
            blocked_link("[click](https://evil.example)", &none).as_deref(),
            Some("evil.example")
        );
        // Mentioning a file name or a version is not a link.
        assert_eq!(blocked_link("update to v1.2.3 in main.rs", &none), None);
    }

    #[test]
    fn allowed_domains_pass() {
        let allowed = vec!["youtube.com".to_owned()];
        assert_eq!(
            blocked_link("https://youtube.com/watch?v=1", &allowed),
            None
        );
        assert_eq!(blocked_link("https://music.youtube.com/x", &allowed), None);
        assert_eq!(
            blocked_link("https://notyoutube.com", &allowed).as_deref(),
            Some("notyoutube.com")
        );
        assert_eq!(
            blocked_link("https://youtube.com https://evil.example", &allowed).as_deref(),
            Some("evil.example")
        );
    }
}
