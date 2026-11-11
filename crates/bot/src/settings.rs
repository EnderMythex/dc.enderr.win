//! `/settings` and `/help`: configuration from Discord.
//!
//! `/settings view` opens a panel: an image of the current configuration, a
//! menu of modules, and for each module buttons and pickers that change it
//! on the spot. Everything here can also be done on the web dashboard.

use dc_core::{
    cards::{self, Row, Tone, Value as Shown},
    guild::GuildConfig,
};

use serde_json::Value;

use crate::{
    app::App,
    interactions::{Ctx, Options},
    logs, panels,
    ui::{self, Msg, Pick},
    welcome,
};

const TEXT_CHANNELS: &[u8] = &[0, 5];
const CATEGORIES: &[u8] = &[4];

/// `(id, name, description)` of the modules in the menu.
const MODULES: &[(&str, &str, &str)] = &[
    (
        "welcome",
        "Welcome",
        "Message, image and roles for new members",
    ),
    (
        "rules",
        "Rules & verification",
        "Rules panel, captcha, verified role",
    ),
    ("tickets", "Tickets", "Private channels with the staff"),
    ("levels", "Levels", "XP, announcements and rewards"),
    ("logs", "Logs", "What is reported in the log channel"),
    ("links", "Link lock", "Block links posted by members"),
    ("admins", "Bot admins", "Who can use the admin commands"),
];

const VERIFY_MODES: &[(&str, &str, &str)] = &[
    ("none", "None", "Rules only, no verification"),
    ("button", "Button", "One click to accept the rules"),
    (
        "captcha",
        "Captcha image",
        "Type the code shown in an image",
    ),
    ("activity", "Activity", "Captcha in a Discord Activity"),
];

const ANNOUNCE_MODES: &[(&str, &str, &str)] = &[
    ("off", "Do not announce", ""),
    ("same", "Where it happened", "In the channel of the message"),
    ("channel", "In a chosen channel", "Pick it below"),
];

const LOG_KINDS: &[(&str, &str, &str)] = &[
    ("message_delete", "Deleted messages", ""),
    (
        "message_edit",
        "Edited messages",
        "With the text before and after",
    ),
    ("channels", "Channels", "Created, deleted, renamed"),
    ("members", "Members", "Arrivals, departures, verifications"),
    (
        "moderation",
        "Moderation",
        "Sanctions, locks, setting changes",
    ),
];

fn label_of(list: &[(&str, &str, &str)], value: &str) -> String {
    list.iter()
        .find(|(id, ..)| *id == value)
        .map_or(value, |(_, label, _)| label)
        .to_owned()
}

fn module_name(id: &str) -> &'static str {
    MODULES
        .iter()
        .find(|(module, ..)| *module == id)
        .map_or("Settings", |(_, name, _)| name)
}

// ---------------------------------------------------------------- image rows

struct Names<'a> {
    app: &'a App,
    guild: &'a str,
}

impl Names<'_> {
    fn channel(&self, id: &Option<String>) -> Shown {
        match id {
            Some(id) => Shown::Chip(self.app.channel_name(self.guild, id)),
            None => Shown::Muted("Not set".into()),
        }
    }

    fn channel_name(&self, id: &Option<String>) -> Option<String> {
        id.as_ref().map(|id| self.app.channel_name(self.guild, id))
    }

    fn role(&self, id: &Option<String>) -> Shown {
        match id {
            Some(id) => Shown::Chip(self.app.role_name(self.guild, id)),
            None => Shown::Muted("None".into()),
        }
    }

    fn roles(&self, ids: &[String]) -> Shown {
        match ids {
            [] => Shown::Muted("None".into()),
            [one] => Shown::Chip(self.app.role_name(self.guild, one)),
            many => Shown::Chip(format!("{} roles", many.len())),
        }
    }
}

fn count(n: usize, one: &str, many: &str) -> Shown {
    match n {
        0 => Shown::Muted("None".into()),
        1 => Shown::Chip(format!("1 {one}")),
        n => Shown::Chip(format!("{n} {many}")),
    }
}

/// Image of the overview: one row per module.
fn home_rows(names: &Names, config: &GuildConfig) -> Vec<Row> {
    let verification = label_of(VERIFY_MODES, &config.rules.verification);
    let admins = &config.admins;
    vec![
        Row::new(
            "Welcome",
            if config.welcome.image {
                "Message with image"
            } else {
                "Message"
            },
            Shown::Module(
                config.welcome.enabled,
                names.channel_name(&config.welcome.channel),
            ),
        ),
        Row::new(
            "Rules & verification",
            &format!("Verification: {verification}"),
            Shown::Module(
                config.rules.enabled,
                names.channel_name(&config.rules.channel),
            ),
        ),
        Row::new(
            "Tickets",
            &format!("{} staff role(s)", config.tickets.staff_roles.len()),
            Shown::Module(
                config.tickets.enabled,
                names.channel_name(&config.tickets.panel_channel),
            ),
        ),
        Row::new(
            "Levels",
            &format!("{}–{} XP per message", config.xp.min_xp, config.xp.max_xp),
            Shown::Module(
                config.xp.enabled,
                names.channel_name(&config.xp.announce_channel),
            ),
        ),
        Row::new(
            "Logs",
            "Deleted and edited messages, members...",
            Shown::Module(
                config.logs.enabled,
                names.channel_name(&config.logs.channel),
            ),
        ),
        Row::new(
            "Link lock",
            &format!(
                "{} allowed website(s)",
                config.lock_url.allowed_domains.len()
            ),
            Shown::Switch(config.lock_url.enabled),
        ),
        Row::new(
            "Bot admins",
            "Besides the owner and Administrators",
            Shown::Chip(format!(
                "{} member(s) · {} role(s)",
                admins.users.len(),
                admins.roles.len()
            )),
        ),
    ]
}

/// Image of one module.
fn module_rows(names: &Names, config: &GuildConfig, module: &str) -> Vec<Row> {
    let on = |enabled: bool| Row::new("Enabled", "", Shown::Switch(enabled));
    match module {
        "welcome" => {
            let w = &config.welcome;
            vec![
                on(w.enabled),
                Row::new(
                    "Channel",
                    "Where new members are welcomed",
                    names.channel(&w.channel),
                ),
                Row::new(
                    "Image",
                    "Avatar, name and member number",
                    Shown::Switch(w.image),
                ),
                Row::new("Roles on arrival", "", names.roles(&w.auto_roles)),
            ]
        }
        "rules" => {
            let r = &config.rules;
            vec![
                on(r.enabled),
                Row::new(
                    "Channel",
                    "Where the rules panel is posted",
                    names.channel(&r.channel),
                ),
                Row::new(
                    "Verification",
                    "",
                    Shown::Chip(label_of(VERIFY_MODES, &r.verification)),
                ),
                Row::new(
                    "Verified role",
                    "Given once verified",
                    names.role(&r.verified_role),
                ),
                Row::new(
                    "Removed role",
                    "Taken once verified",
                    names.role(&r.remove_role),
                ),
            ]
        }
        "tickets" => {
            let t = &config.tickets;
            vec![
                on(t.enabled),
                Row::new("Panel channel", "", names.channel(&t.panel_channel)),
                Row::new(
                    "Category",
                    "Where tickets are created",
                    names.channel(&t.category),
                ),
                Row::new("Staff roles", "", names.roles(&t.staff_roles)),
                Row::new(
                    "Transcripts",
                    "Sent when a ticket closes",
                    names.channel(&t.log_channel),
                ),
                Row::new("Topics", "", count(t.topics.len(), "topic", "topics")),
            ]
        }
        "levels" => {
            let x = &config.xp;
            vec![
                on(x.enabled),
                Row::new(
                    "XP per message",
                    &format!("Once every {} s", x.cooldown_secs),
                    Shown::Chip(format!("{}–{} XP", x.min_xp, x.max_xp)),
                ),
                Row::new(
                    "Announcements",
                    "",
                    Shown::Chip(label_of(ANNOUNCE_MODES, &x.announce)),
                ),
                Row::new(
                    "Announcement channel",
                    "",
                    names.channel(&x.announce_channel),
                ),
                Row::new(
                    "Role rewards",
                    "",
                    count(x.rewards.len(), "reward", "rewards"),
                ),
            ]
        }
        "logs" => {
            let l = &config.logs;
            vec![
                on(l.enabled),
                Row::new(
                    "Channel",
                    "Keep it private to the staff",
                    names.channel(&l.channel),
                ),
                Row::new("Deleted messages", "", Shown::Switch(l.message_delete)),
                Row::new("Edited messages", "", Shown::Switch(l.message_edit)),
                Row::new("Channels", "", Shown::Switch(l.channels)),
                Row::new("Members", "", Shown::Switch(l.members)),
                Row::new("Moderation", "", Shown::Switch(l.moderation)),
            ]
        }
        "links" => {
            let l = &config.lock_url;
            vec![
                on(l.enabled),
                Row::new(
                    "Allowed websites",
                    "Edited on the dashboard",
                    count(l.allowed_domains.len(), "website", "websites"),
                ),
                Row::new("Exempt roles", "", names.roles(&l.exempt_roles)),
                Row::new(
                    "Exempt channels",
                    "",
                    count(l.exempt_channels.len(), "channel", "channels"),
                ),
            ]
        }
        _ => {
            let a = &config.admins;
            vec![
                Row::new(
                    "Members",
                    "Added here or with /ownerbot",
                    count(a.users.len(), "member", "members"),
                ),
                Row::new("Roles", "Every member with the role", names.roles(&a.roles)),
            ]
        }
    }
}

// ---------------------------------------------------------------- panel

fn module_menu(current: Option<&str>) -> Value {
    let selected: Vec<&str> = current.into_iter().collect();
    ui::row(vec![ui::string_select(
        "settings:module",
        "Choose a module to configure",
        MODULES,
        &selected,
        1,
    )])
}

/// Small caption above a picker, which otherwise has no label once filled.
fn labeled(caption: &str, picker: Value) -> [Value; 2] {
    [ui::text(format!("-# {caption}")), ui::row(vec![picker])]
}

fn toggle(enabled: bool, custom_id: &str, what: &str) -> Value {
    if enabled {
        ui::button(ui::STYLE_SECONDARY, &format!("Turn {what} off"), custom_id)
    } else {
        ui::button(ui::STYLE_SUCCESS, &format!("Turn {what} on"), custom_id)
    }
}

fn dashboard_button(ctx: &Ctx) -> Option<Value> {
    let url = ctx.app.dashboard_url.as_ref()?;
    let link = format!("{}/servers/{}", url.trim_end_matches('/'), ctx.guild_id);
    Some(ui::link_button("Dashboard", &link))
}

fn ids(id: &Option<String>) -> Vec<String> {
    id.iter().cloned().collect()
}

/// Buttons and pickers of one module.
fn module_controls(config: &GuildConfig, module: &str) -> Vec<Value> {
    let mut buttons = Vec::new();
    let mut out = Vec::new();
    let channel = |id: &str, caption: &str, current: &Option<String>| {
        labeled(
            caption,
            ui::entity_select(
                &format!("settings:channel:{id}"),
                caption,
                Pick::Channels(TEXT_CHANNELS),
                &ids(current),
                1,
            ),
        )
    };
    let roles = |id: &str, caption: &str, current: &[String], max: usize| {
        labeled(
            caption,
            ui::entity_select(
                &format!("settings:roles:{id}"),
                caption,
                Pick::Roles,
                current,
                max,
            ),
        )
    };

    match module {
        "welcome" => {
            let w = &config.welcome;
            buttons.push(toggle(w.enabled, "settings:toggle:welcome", "welcome"));
            buttons.push(toggle(
                w.image,
                "settings:toggle:welcome_image",
                "the image",
            ));
            buttons.push(ui::button(
                ui::STYLE_SECONDARY,
                "Preview",
                "settings:preview",
            ));
            out.extend(channel("welcome", "Welcome channel", &w.channel));
            out.extend(roles(
                "welcome",
                "Roles given on arrival",
                &w.auto_roles,
                10,
            ));
        }
        "rules" => {
            let r = &config.rules;
            buttons.push(toggle(r.enabled, "settings:toggle:rules", "the panel"));
            out.extend(channel("rules", "Rules channel", &r.channel));
            out.extend(labeled(
                "Verification",
                ui::string_select(
                    "settings:mode:verification",
                    "Verification",
                    VERIFY_MODES,
                    &[r.verification.as_str()],
                    1,
                ),
            ));
            out.extend(roles(
                "verified",
                "Verified role",
                &ids(&r.verified_role),
                1,
            ));
            out.extend(roles(
                "unverified",
                "Role removed once verified",
                &ids(&r.remove_role),
                1,
            ));
        }
        "tickets" => {
            let t = &config.tickets;
            buttons.push(toggle(t.enabled, "settings:toggle:tickets", "tickets"));
            out.extend(channel("tickets", "Panel channel", &t.panel_channel));
            out.extend(labeled(
                "Category of the ticket channels",
                ui::entity_select(
                    "settings:channel:ticket_category",
                    "Category of the ticket channels",
                    Pick::Channels(CATEGORIES),
                    &ids(&t.category),
                    1,
                ),
            ));
            out.extend(roles("staff", "Staff roles", &t.staff_roles, 10));
            out.extend(channel("transcripts", "Transcript channel", &t.log_channel));
        }
        "levels" => {
            let x = &config.xp;
            buttons.push(toggle(x.enabled, "settings:toggle:levels", "levels"));
            out.extend(labeled(
                "Level-up announcements",
                ui::string_select(
                    "settings:mode:announce",
                    "Level-up announcements",
                    ANNOUNCE_MODES,
                    &[x.announce.as_str()],
                    1,
                ),
            ));
            out.extend(channel(
                "announce",
                "Announcement channel",
                &x.announce_channel,
            ));
        }
        "logs" => {
            let l = &config.logs;
            buttons.push(toggle(l.enabled, "settings:toggle:logs", "logs"));
            out.extend(channel("logs", "Log channel", &l.channel));
            let on: Vec<&str> = [
                (l.message_delete, "message_delete"),
                (l.message_edit, "message_edit"),
                (l.channels, "channels"),
                (l.members, "members"),
                (l.moderation, "moderation"),
            ]
            .into_iter()
            .filter_map(|(enabled, id)| enabled.then_some(id))
            .collect();
            out.extend(labeled(
                "What is logged",
                ui::string_select(
                    "settings:logkinds",
                    "Nothing is logged",
                    LOG_KINDS,
                    &on,
                    LOG_KINDS.len(),
                ),
            ));
        }
        "links" => {
            let l = &config.lock_url;
            buttons.push(toggle(l.enabled, "settings:toggle:links", "link lock"));
            out.extend(roles(
                "link_exempt",
                "Roles allowed to post links",
                &l.exempt_roles,
                10,
            ));
            out.push(ui::text(
                "-# Allowed websites and exempt channels are set on the dashboard.",
            ));
        }
        _ => {
            let a = &config.admins;
            out.extend(labeled(
                "Admin members",
                ui::entity_select(
                    "settings:users:admins",
                    "Admin members",
                    Pick::Users,
                    &a.users,
                    25,
                ),
            ));
            out.extend(roles("admins", "Admin roles", &a.roles, 10));
        }
    }
    let mut rows = Vec::new();
    if !buttons.is_empty() {
        rows.push(ui::row(buttons));
    }
    rows.extend(out);
    rows
}

/// The panel: `None` for the overview, else one module.
fn panel(ctx: &Ctx, config: &GuildConfig, module: Option<&str>) -> Msg {
    let names = Names {
        app: &ctx.app,
        guild: &ctx.guild_id,
    };
    let label = format!("{} · Settings", ctx.app.name);
    let (title, description, rows) = match module {
        None => (
            "Settings",
            "Choose a module in the menu below to change it. Everything is also on the dashboard.",
            home_rows(&names, config),
        ),
        Some(module) => (
            module_name(module),
            MODULES
                .iter()
                .find(|(id, ..)| *id == module)
                .map_or("", |(_, _, description)| description),
            module_rows(&names, config, module),
        ),
    };
    let svg = cards::settings(&label, title, description, &rows);

    let mut children = vec![ui::separator()];
    if let Some(module) = module {
        children.extend(module_controls(config, module));
        children.push(ui::separator());
    }
    children.push(module_menu(module));
    let mut nav = Vec::new();
    if module.is_some() {
        nav.push(ui::button(ui::STYLE_SECONDARY, "Back", "settings:home"));
    }
    nav.extend(dashboard_button(ctx));
    if !nav.is_empty() {
        children.push(ui::row(nav));
    }
    let fallback = vec![ui::text(format!("### {title}\n{description}"))];
    Msg::with_image(ui::WHITE, "settings.png", svg, children, fallback).ephemeral()
}

// ---------------------------------------------------------------- slash command

pub async fn command(ctx: &Ctx, options: &Options) {
    let sub = options.sub.as_deref().unwrap_or("view");
    if sub == "view" {
        ctx.defer(true).await;
        return ctx.reply(panel(ctx, &ctx.config(), None)).await;
    }
    if sub == "preview" {
        ctx.defer(true).await;
        let msg = welcome::message(
            &ctx.app,
            &ctx.guild_id,
            &ctx.config(),
            &ctx.user,
            &ctx.member,
        )
        .await;
        return ctx.reply(msg).await;
    }

    let enabled = options.bool("enabled");
    let id = |name: &str| options.str(name).map(str::to_owned);
    let module = match sub {
        "xp" => "levels",
        other => other,
    };
    let saved = ctx.app.update_config(&ctx.guild_id, |config| match sub {
        "welcome" => {
            let welcome = &mut config.welcome;
            if let Some(on) = enabled {
                welcome.enabled = on;
            }
            if let Some(channel) = id("channel") {
                welcome.channel = Some(channel);
            }
            if let Some(message) = id("message") {
                welcome.message = message;
            }
            if let Some(image) = options.bool("image") {
                welcome.image = image;
            }
            if let Some(role) = id("role") {
                welcome.auto_roles = vec![role];
            }
        }
        "rules" => {
            let rules = &mut config.rules;
            if let Some(on) = enabled {
                rules.enabled = on;
            }
            if let Some(channel) = id("channel") {
                rules.channel = Some(channel);
            }
            if let Some(mode) = id("verification") {
                rules.verification = mode;
            }
            if let Some(role) = id("role") {
                rules.verified_role = Some(role);
            }
        }
        "tickets" => {
            let tickets = &mut config.tickets;
            if let Some(on) = enabled {
                tickets.enabled = on;
            }
            if let Some(channel) = id("channel") {
                tickets.panel_channel = Some(channel);
            }
            if let Some(category) = id("category") {
                tickets.category = Some(category);
            }
            if let Some(role) = id("staff_role")
                && !tickets.staff_roles.contains(&role)
            {
                tickets.staff_roles.push(role);
            }
            if let Some(channel) = id("log_channel") {
                tickets.log_channel = Some(channel);
            }
        }
        "xp" => {
            if let Some(on) = enabled {
                config.xp.enabled = on;
            }
            if let Some(channel) = id("announce_channel") {
                config.xp.announce = "channel".into();
                config.xp.announce_channel = Some(channel);
            }
        }
        "logs" => {
            if let Some(on) = enabled {
                config.logs.enabled = on;
            }
            if let Some(channel) = id("channel") {
                config.logs.channel = Some(channel);
            }
        }
        _ => {}
    });

    let config = match saved {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "saving the configuration failed");
            return ctx
                .reply(Msg::error("The settings could not be saved."))
                .await;
        }
    };
    ctx.defer(true).await;
    ctx.reply(panel(ctx, &config, Some(module))).await;
    after_change(ctx, module).await;
}

/// Puts the rules and ticket panels in line with the new settings, and logs
/// who changed what.
async fn after_change(ctx: &Ctx, module: &str) {
    if matches!(module, "rules" | "tickets") {
        let (app, guild) = (ctx.app.clone(), ctx.guild_id.clone());
        tokio::spawn(async move { panels::sync(&app, &guild).await });
    }
    let entry = format!(
        "<@{}> changed the **{}** settings from Discord.",
        ctx.user_id(),
        module_name(module)
    );
    logs::moderation(&ctx.app, &ctx.guild_id, "Settings changed", &entry).await;
}

// ---------------------------------------------------------------- panel clicks

/// The module a control belongs to.
fn module_of(target: &str) -> &'static str {
    match target {
        "welcome" | "welcome_image" => "welcome",
        "rules" | "verified" | "unverified" | "verification" => "rules",
        "tickets" | "ticket_category" | "staff" | "transcripts" => "tickets",
        "levels" | "announce" => "levels",
        "logs" | "logkinds" => "logs",
        "links" | "link_exempt" => "links",
        _ => "admins",
    }
}

/// Applies one click or pick to the configuration; `None` when the
/// control is unknown.
fn apply(config: &mut GuildConfig, kind: &str, target: &str, values: &[String]) -> Option<()> {
    let first = values.first().cloned();
    match (kind, target) {
        ("toggle", "welcome") => config.welcome.enabled ^= true,
        ("toggle", "welcome_image") => config.welcome.image ^= true,
        ("toggle", "rules") => config.rules.enabled ^= true,
        ("toggle", "tickets") => config.tickets.enabled ^= true,
        ("toggle", "levels") => config.xp.enabled ^= true,
        ("toggle", "logs") => config.logs.enabled ^= true,
        ("toggle", "links") => config.lock_url.enabled ^= true,

        ("channel", "welcome") => config.welcome.channel = first,
        ("channel", "rules") => config.rules.channel = first,
        ("channel", "tickets") => config.tickets.panel_channel = first,
        ("channel", "ticket_category") => config.tickets.category = first,
        ("channel", "transcripts") => config.tickets.log_channel = first,
        ("channel", "logs") => config.logs.channel = first,
        ("channel", "announce") => {
            if first.is_some() {
                config.xp.announce = "channel".into();
            }
            config.xp.announce_channel = first;
        }

        ("roles", "welcome") => config.welcome.auto_roles = values.to_vec(),
        ("roles", "verified") => config.rules.verified_role = first,
        ("roles", "unverified") => config.rules.remove_role = first,
        ("roles", "staff") => config.tickets.staff_roles = values.to_vec(),
        ("roles", "link_exempt") => config.lock_url.exempt_roles = values.to_vec(),
        ("roles", "admins") => config.admins.roles = values.to_vec(),
        ("users", "admins") => config.admins.users = values.to_vec(),

        ("mode", "verification") => config.rules.verification = first?,
        ("mode", "announce") => config.xp.announce = first?,
        ("logkinds", _) => {
            let has = |id: &str| values.iter().any(|v| v == id);
            let logs = &mut config.logs;
            logs.message_delete = has("message_delete");
            logs.message_edit = has("message_edit");
            logs.channels = has("channels");
            logs.members = has("members");
            logs.moderation = has("moderation");
        }
        _ => return None,
    }
    Some(())
}

/// Buttons and menus of the settings panel.
pub async fn component(ctx: &Ctx) {
    if !ctx.is_admin() {
        return ctx.reply(Msg::denied()).await;
    }
    let id = ctx.custom_id().to_owned();
    let mut parts = id.split(':').skip(1);
    let (kind, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let values = ctx.values();

    match kind {
        "home" => return ctx.update(panel(ctx, &ctx.config(), None)).await,
        "module" => {
            let module = values.first().map_or("welcome", String::as_str);
            let module = MODULES
                .iter()
                .find(|(id, ..)| *id == module)
                .map_or("welcome", |(id, ..)| id);
            return ctx.update(panel(ctx, &ctx.config(), Some(module))).await;
        }
        "preview" => {
            ctx.defer(true).await;
            let msg = welcome::message(
                &ctx.app,
                &ctx.guild_id,
                &ctx.config(),
                &ctx.user,
                &ctx.member,
            )
            .await;
            return ctx.reply(msg).await;
        }
        _ => {}
    }

    // Only the server owner and the bot's owners may change the admins.
    if target == "admins" && !ctx.app.is_super_admin(&ctx.guild_id, ctx.user_id()) {
        return ctx
            .reply(Msg::denied_with(
                "Only the server owner and the bot's owners can change the bot admins.",
            ))
            .await;
    }

    let module = if kind == "logkinds" {
        "logs"
    } else {
        module_of(target)
    };
    let mut known = true;
    let saved = ctx.app.update_config(&ctx.guild_id, |config| {
        known = apply(config, kind, target, &values).is_some();
    });
    match saved {
        Ok(config) if known => {
            ctx.update(panel(ctx, &config, Some(module))).await;
            after_change(ctx, module).await;
        }
        Ok(_) => {
            ctx.reply(Msg::error("This control is no longer active."))
                .await
        }
        Err(error) => {
            tracing::error!(%error, "saving the configuration failed");
            ctx.reply(Msg::error("The settings could not be saved."))
                .await;
        }
    }
}

// ---------------------------------------------------------------- help

pub async fn help(ctx: &Ctx) {
    let everyone = "`/rank` · your level and XP\n`/leaderboard` · the most active members";
    let mut children = vec![ui::text(everyone)];
    if ctx.is_admin() {
        let sections = [
            "**Channels**\n`/lock` `/unlock` · stop or allow writing\n`/hide` `/unhide` · hide or show a channel\n`/lockurl` · block links",
            "**Members**\n`/mute` `/unmute` · timeout\n`/kick` `/ban` · asks for confirmation\n`/unban`\n`/role add` `/role remove`\n`/role add-all` `/role remove-all` · every member\n`/xp add` `/xp remove` `/xp reset`",
            "**Giveaways**\n`/giveaway start` · prize, duration, winners\n`/giveaway end` `/giveaway reroll` `/giveaway cancel` `/giveaway list`",
            "**Configuration**\n`/settings view` · the settings panel\n`/settings welcome|rules|tickets|xp|logs`\n`/ownerbot add|remove|list` · bot admins",
        ];
        for section in sections {
            children.push(ui::separator());
            children.push(ui::text(section));
        }
    }
    let msg = Msg::banner(
        Tone::Info,
        "Commands",
        "Everything this bot can do for you on this server.",
        children,
    );
    ctx.reply(msg.ephemeral()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "175928847299117063";

    fn applied(kind: &str, target: &str, values: &[&str]) -> Option<GuildConfig> {
        let mut config = GuildConfig::default();
        let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        apply(&mut config, kind, target, &values)?;
        config.sanitize();
        Some(config)
    }

    #[test]
    fn controls_change_the_right_setting() {
        assert!(applied("toggle", "welcome", &[]).unwrap().welcome.enabled);
        assert!(
            !applied("toggle", "welcome_image", &[])
                .unwrap()
                .welcome
                .image
        );
        let config = applied("channel", "rules", &[ID]).unwrap();
        assert_eq!(config.rules.channel.as_deref(), Some(ID));
        // An emptied picker clears the setting.
        assert_eq!(
            applied("channel", "rules", &[]).unwrap().rules.channel,
            None
        );
        let config = applied("channel", "announce", &[ID]).unwrap();
        assert_eq!(config.xp.announce, "channel");
        let config = applied("mode", "verification", &["activity"]).unwrap();
        assert_eq!(config.rules.verification, "activity");
        // Bad values are fixed by sanitize, as for the dashboard.
        let config = applied("mode", "verification", &["magic"]).unwrap();
        assert_eq!(config.rules.verification, "button");
        let config = applied("logkinds", "", &["members"]).unwrap();
        assert!(config.logs.members && !config.logs.message_delete);
        let config = applied("users", "admins", &[ID, "x"]).unwrap();
        assert_eq!(config.admins.users, vec![ID]);
        assert!(applied("toggle", "nothing", &[]).is_none());
    }

    /// Discord refuses a message with more than 40 components in total, a
    /// row mixing a menu with anything else, or more than 5 buttons in a row.
    #[test]
    fn module_pages_respect_discord_limits() {
        fn count(component: &Value) -> usize {
            let children = component["components"].as_array().into_iter().flatten();
            1 + children.map(count).sum::<usize>()
        }
        fn check_rows(component: &Value) {
            if component["type"] == 1 {
                let items = component["components"].as_array().unwrap();
                let menus = items.iter().filter(|c| c["type"] != 2).count();
                assert!(
                    menus == 0 || items.len() == 1,
                    "menu shares a row: {component}"
                );
                assert!(items.len() <= 5, "too many buttons: {component}");
            }
            for child in component["components"].as_array().into_iter().flatten() {
                check_rows(child);
            }
        }
        let mut config = GuildConfig::default();
        config.admins.users = (0..30).map(|i| format!("17592884729911{i:04}")).collect();
        for (module, ..) in MODULES {
            let controls = module_controls(&config, module);
            // container + image + separators + module menu + navigation row.
            let overhead = 1 + 1 + 2 + 2 + 3;
            let total: usize = controls.iter().map(count).sum::<usize>() + overhead;
            assert!(total <= 40, "{module}: {total} components");
            for control in &controls {
                check_rows(control);
                for picker in control["components"].as_array().into_iter().flatten() {
                    let defaults = picker["default_values"].as_array().map_or(0, Vec::len);
                    let max = picker["max_values"].as_u64().unwrap_or(25) as usize;
                    assert!(defaults <= max, "{module}: {picker}");
                }
            }
        }
        check_rows(&module_menu(Some("welcome")));
    }

    #[test]
    fn every_control_belongs_to_a_module() {
        let config = GuildConfig::default();
        for (module, ..) in MODULES {
            for control in module_controls(&config, module) {
                let text = control.to_string();
                for id in text.split("\"custom_id\":\"").skip(1) {
                    let id = id.split('"').next().unwrap();
                    let target = id.split(':').nth(2).unwrap_or("");
                    if id.starts_with("settings:toggle") || id.starts_with("settings:channel") {
                        assert_eq!(module_of(target), *module, "{id}");
                    }
                }
            }
        }
    }
}
