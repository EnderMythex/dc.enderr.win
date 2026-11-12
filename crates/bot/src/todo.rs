//! Tasks of the staff (Misty pack): a panel listing the open tasks, each
//! with a Claim button on its right, then Finish once claimed; an "Add a
//! task" button opening a form; and `/todo add|claim|unclaim|finish|remove|list`.
//!
//! A section of Components V2 has room for one button on the right of its
//! text, so a task shows the next step: Claim while nobody has it, Finish
//! once someone does.

use dc_core::{cards::Tone, guild::GuildConfig};

use crate::{
    app::{App, roles_of},
    db::{self, Todo},
    interactions::{Ctx, Options, modal_value},
    logs, panels,
    ui::{self, Field, Msg},
};

pub const PREFIX: &str = "todo:";
/// Kind of the panel in the `panels` table (and section of the dashboard).
pub const KIND: &str = "todo";
const ADD: &str = "todo:add";
pub const FORM: &str = "todo:new";

const MAX_TITLE: usize = 100;
const MAX_DETAILS: usize = 400;
/// Tasks listed on the panel; a message holds at most 40 components.
const MAX_SHOWN: usize = 7;
/// Finished tasks listed under the open ones.
const DONE_SHOWN: i64 = 3;

// ---------------------------------------------------------------- panel

/// Text of one task: number and title, details, then who has it.
fn line(todo: &Todo) -> String {
    let mut text = format!("**#{} · {}**", todo.number, ui::escape(&todo.title));
    if !todo.details.is_empty() {
        text.push('\n');
        text.push_str(&ui::truncate(&todo.details, 200));
    }
    match (&todo.claimed_by, todo.claimed_at) {
        (Some(user), Some(at)) => text.push_str(&format!("\n-# Claimed by <@{user}> <t:{at}:R>")),
        (Some(user), None) => text.push_str(&format!("\n-# Claimed by <@{user}>")),
        _ => text.push_str(&format!(
            "\n-# Open · added by <@{}> <t:{}:R>",
            todo.author_id, todo.created_at
        )),
    }
    text
}

/// The button on the right of a task: its next step.
fn action(todo: &Todo) -> serde_json::Value {
    if todo.claimed_by.is_some() {
        ui::emoji_button(
            ui::STYLE_SUCCESS,
            "✅",
            "Finish",
            &format!("todo:finish:{}", todo.number),
        )
    } else {
        ui::emoji_button(
            ui::STYLE_PRIMARY,
            "🙋",
            "Claim",
            &format!("todo:claim:{}", todo.number),
        )
    }
}

/// The panel: the open tasks (oldest first), the last finished ones, and
/// the Add button. Mentions in it never ping anyone.
pub fn panel(config: &GuildConfig, open: &[Todo], done: &[Todo]) -> Msg {
    let settings = &config.todo;
    let mut children = Vec::new();
    if !settings.text.is_empty() {
        children.push(ui::text(settings.text.clone()));
    }
    if open.is_empty() {
        children.push(ui::text(
            "Nothing to do right now. Add a task with the button below or `/todo add`.",
        ));
    }
    for todo in open.iter().take(MAX_SHOWN) {
        children.push(ui::separator());
        children.push(ui::section_button(line(todo), action(todo)));
    }
    if open.len() > MAX_SHOWN {
        children.push(ui::text(format!(
            "-# And {} more: `/todo list` shows every task.",
            open.len() - MAX_SHOWN
        )));
    }
    if settings.show_done && !done.is_empty() {
        let lines: Vec<String> = done
            .iter()
            .map(|todo| {
                let by = todo
                    .done_by
                    .as_ref()
                    .map(|user| format!(" · <@{user}>"))
                    .unwrap_or_default();
                let at = todo
                    .done_at
                    .map(|at| format!(" <t:{at}:R>"))
                    .unwrap_or_default();
                format!(
                    "-# ✅ ~~#{} {}~~{by}{at}",
                    todo.number,
                    ui::escape(&ui::truncate(&todo.title, 60))
                )
            })
            .collect();
        children.push(ui::separator());
        children.push(ui::text(format!("**Done recently**\n{}", lines.join("\n"))));
    }
    children.push(ui::separator());
    children.push(ui::row(vec![ui::emoji_button(
        ui::STYLE_SECONDARY,
        "➕",
        "Add a task",
        ADD,
    )]));
    ui::panel_card(&settings.look, &settings.title, children)
}

/// Makes the panel of a server match its tasks and settings.
pub async fn sync(app: &App, guild_id: &str) {
    let config = app.config(guild_id);
    let Some(channel) = config.todo.channel.clone().filter(|_| config.todo.enabled) else {
        return panels::sync_one(app, guild_id, KIND, None).await;
    };
    let tasks = async {
        let open = db::open_todos(&app.db, guild_id).await?;
        let done = if config.todo.show_done {
            db::done_todos(&app.db, guild_id, DONE_SHOWN).await?
        } else {
            Vec::new()
        };
        Ok::<_, sqlx::Error>((open, done))
    };
    match tasks.await {
        Ok((open, done)) => {
            let msg = panel(&config, &open, &done);
            panels::sync_one(app, guild_id, KIND, Some((channel, msg))).await;
        }
        // Leave the panel as it is rather than taking it down.
        Err(error) => tracing::error!(%error, "reading the tasks failed"),
    }
}

// ---------------------------------------------------------------- actions

/// Bot admins and the staff roles of the section.
fn is_staff(ctx: &Ctx) -> bool {
    let roles = roles_of(&ctx.member);
    ctx.config()
        .todo
        .staff_roles
        .iter()
        .any(|role| roles.contains(role))
        || ctx.is_admin()
}

/// What a change does: `Ok` says what happened, `Err` why nothing did.
type Outcome = Result<String, String>;

async fn open_task(app: &App, guild: &str, number: i64) -> Result<Todo, String> {
    match db::get_todo(&app.db, guild, number).await {
        Ok(Some(todo)) if !todo.done => Ok(todo),
        Ok(Some(_)) => Err(format!("Task #{number} is already done.")),
        Ok(None) => Err(format!(
            "There is no task #{number}. `/todo list` shows them."
        )),
        Err(error) => {
            tracing::error!(%error, "reading a task failed");
            Err("Try again in a moment.".into())
        }
    }
}

fn failed(error: sqlx::Error) -> String {
    tracing::error!(%error, "saving a task failed");
    "The task could not be saved. Try again in a moment.".into()
}

async fn add(ctx: &Ctx, title: &str, details: &str, assign: Option<&str>) -> Outcome {
    let title = ui::truncate(title.trim(), MAX_TITLE);
    if title.is_empty() {
        return Err("Describe the task.".into());
    }
    let details = ui::truncate(details.trim(), MAX_DETAILS);
    let number = db::create_todo(
        &ctx.app.db,
        &ctx.guild_id,
        (&title, &details),
        ctx.user_id(),
        assign,
    )
    .await
    .map_err(failed)?;
    let mut body = format!("**Task:** #{number} {}", ui::escape(&title));
    if let Some(user) = assign {
        body.push_str(&format!("\n**Claimed by:** <@{user}>"));
    }
    body.push_str(&format!("\n**By:** <@{}>", ctx.user_id()));
    logs::moderation(&ctx.app, &ctx.guild_id, "Task added", &body).await;
    Ok(match assign {
        Some(user) => format!("Task #{number} added and claimed by <@{user}>."),
        None => format!("Task #{number} added to the panel."),
    })
}

async fn claim(ctx: &Ctx, number: i64) -> Outcome {
    let todo = open_task(&ctx.app, &ctx.guild_id, number).await?;
    if let Some(user) = &todo.claimed_by {
        return Err(if user == ctx.user_id() {
            format!("You already have task #{number}.")
        } else {
            format!("<@{user}> already claimed task #{number}.")
        });
    }
    match db::claim_todo(&ctx.app.db, &ctx.guild_id, number, ctx.user_id()).await {
        Ok(true) => Ok(format!("Task #{number} is yours.")),
        Ok(false) => Err(format!("Someone claimed task #{number} just before you.")),
        Err(error) => Err(failed(error)),
    }
}

/// The member who claimed a task, or a bot admin.
fn may_change(ctx: &Ctx, todo: &Todo) -> bool {
    todo.claimed_by
        .as_deref()
        .is_none_or(|user| user == ctx.user_id())
        || ctx.is_admin()
}

async fn unclaim(ctx: &Ctx, number: i64) -> Outcome {
    let todo = open_task(&ctx.app, &ctx.guild_id, number).await?;
    let Some(user) = &todo.claimed_by else {
        return Err(format!("Nobody has claimed task #{number}."));
    };
    if !may_change(ctx, &todo) {
        return Err(format!(
            "Only <@{user}> or a bot admin can give task #{number} back."
        ));
    }
    db::unclaim_todo(&ctx.app.db, &ctx.guild_id, number)
        .await
        .map_err(failed)?;
    Ok(format!("Task #{number} is open again."))
}

async fn finish(ctx: &Ctx, number: i64) -> Outcome {
    let todo = open_task(&ctx.app, &ctx.guild_id, number).await?;
    if !may_change(ctx, &todo) {
        let user = todo.claimed_by.as_deref().unwrap_or_default();
        return Err(format!(
            "<@{user}> claimed task #{number}: only they or a bot admin can finish it."
        ));
    }
    match db::finish_todo(&ctx.app.db, &ctx.guild_id, number, ctx.user_id()).await {
        Ok(true) => {
            let body = format!(
                "**Task:** #{number} {}\n**By:** <@{}>",
                ui::escape(&todo.title),
                ctx.user_id()
            );
            logs::moderation(&ctx.app, &ctx.guild_id, "Task finished", &body).await;
            Ok(format!("Task #{number} is done. Well done!"))
        }
        Ok(false) => Err(format!("Task #{number} is already done.")),
        Err(error) => Err(failed(error)),
    }
}

async fn remove(ctx: &Ctx, number: i64) -> Outcome {
    let todo = match db::get_todo(&ctx.app.db, &ctx.guild_id, number).await {
        Ok(Some(todo)) => todo,
        Ok(None) => return Err(format!("There is no task #{number}.")),
        Err(error) => return Err(failed(error)),
    };
    if todo.author_id != ctx.user_id() && !ctx.is_admin() {
        return Err(format!(
            "Only <@{}>, who added task #{number}, or a bot admin can delete it.",
            todo.author_id
        ));
    }
    db::delete_todo(&ctx.app.db, &ctx.guild_id, number)
        .await
        .map_err(failed)?;
    let body = format!(
        "**Task:** #{number} {}\n**By:** <@{}>",
        ui::escape(&todo.title),
        ctx.user_id()
    );
    logs::moderation(&ctx.app, &ctx.guild_id, "Task deleted", &body).await;
    Ok(format!("Task #{number} was deleted."))
}

fn refused(reason: &str) -> Msg {
    Msg::banner(Tone::Warning, "Staff tasks", reason, Vec::new()).ephemeral()
}

fn denied() -> Msg {
    Msg::denied_with("Only the staff can manage these tasks.")
}

// ---------------------------------------------------------------- Discord

/// Claim, Finish and Add buttons of the panel.
pub async fn component(ctx: &Ctx) {
    if !is_staff(ctx) {
        return ctx.reply(denied()).await;
    }
    let id = ctx.custom_id();
    if id == ADD {
        return ctx.modal(form()).await;
    }
    let mut parts = id.split(':').skip(1);
    let action = parts.next().unwrap_or_default();
    let Some(number) = parts.next().and_then(|n| n.parse::<i64>().ok()) else {
        return ctx
            .reply(Msg::error("This button is no longer active."))
            .await;
    };
    let outcome = match action {
        "claim" => claim(ctx, number).await,
        "finish" => finish(ctx, number).await,
        _ => Err("This button is no longer active.".into()),
    };
    match outcome {
        // The panel itself shows the change.
        Ok(_) => {
            ctx.defer_update().await;
            sync(&ctx.app, &ctx.guild_id).await;
        }
        Err(reason) => {
            ctx.reply(refused(&reason)).await;
            // The panel may be out of date (a task finished elsewhere).
            sync(&ctx.app, &ctx.guild_id).await;
        }
    }
}

fn form() -> serde_json::Value {
    ui::form_modal(
        FORM,
        "New task",
        &[
            Field {
                id: "task",
                label: "Task",
                placeholder: "Pizza module on Misty Bot",
                max: MAX_TITLE as u32,
                paragraph: false,
                required: true,
            },
            Field {
                id: "details",
                label: "Details",
                placeholder: "Optional",
                max: MAX_DETAILS as u32,
                paragraph: true,
                required: false,
            },
        ],
    )
}

/// The form of the Add button.
pub async fn submitted(ctx: &Ctx) {
    if !is_staff(ctx) {
        return ctx.reply(denied()).await;
    }
    let title = modal_value(&ctx.data, "task").to_owned();
    let details = modal_value(&ctx.data, "details").to_owned();
    ctx.defer(true).await;
    match add(ctx, &title, &details, None).await {
        Ok(text) => {
            sync(&ctx.app, &ctx.guild_id).await;
            ctx.reply(Msg::success("Task added", &text)).await;
        }
        Err(reason) => ctx.reply(refused(&reason)).await,
    }
}

/// `/todo ...` (staff of the section and bot admins).
pub async fn command(ctx: &Ctx, options: &Options) {
    if !is_staff(ctx) {
        return ctx.reply(denied()).await;
    }
    let number = options.int("id").unwrap_or(0);
    let sub = options.sub.as_deref().unwrap_or("list");
    if sub == "list" {
        return list(ctx).await;
    }
    ctx.defer(true).await;
    let outcome = match sub {
        "add" => {
            let assign = options.str("assign");
            add(
                ctx,
                options.str("task").unwrap_or_default(),
                options.str("details").unwrap_or_default(),
                assign,
            )
            .await
        }
        "claim" => claim(ctx, number).await,
        "unclaim" => unclaim(ctx, number).await,
        "finish" => finish(ctx, number).await,
        _ => remove(ctx, number).await,
    };
    match outcome {
        Ok(text) => {
            sync(&ctx.app, &ctx.guild_id).await;
            let config = ctx.config();
            let text = if config.todo.enabled && config.todo.channel.is_some() {
                text
            } else {
                format!(
                    "{text}\n-# The panel is off: turn it on in the Staff tasks section of the dashboard."
                )
            };
            ctx.reply(Msg::success("Staff tasks", &text)).await;
        }
        Err(reason) => ctx.reply(refused(&reason)).await,
    }
}

/// Every open task, the member's own first.
async fn list(ctx: &Ctx) {
    let mut open = match db::open_todos(&ctx.app.db, &ctx.guild_id).await {
        Ok(open) => open,
        Err(error) => {
            tracing::error!(%error, "reading the tasks failed");
            return ctx.reply(Msg::error("Try again in a moment.")).await;
        }
    };
    if open.is_empty() {
        let text = "Nothing to do right now. Add a task with /todo add.";
        return ctx
            .reply(Msg::banner(Tone::Info, "Staff tasks", text, Vec::new()).ephemeral())
            .await;
    }
    let me = ctx.user_id().to_owned();
    open.sort_by_key(|todo| (todo.claimed_by.as_deref() != Some(me.as_str()), todo.number));
    let lines: Vec<String> = open
        .iter()
        .map(|todo| {
            let who = match &todo.claimed_by {
                Some(user) if *user == me => "**yours**".to_owned(),
                Some(user) => format!("<@{user}>"),
                None => "open".to_owned(),
            };
            format!(
                "**#{}** {} · {who}",
                todo.number,
                ui::escape(&ui::truncate(&todo.title, 80))
            )
        })
        .collect();
    let msg = Msg::banner(Tone::Info, "Staff tasks", &lines.join("\n"), Vec::new());
    ctx.reply(msg.ephemeral()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(number: i64, claimed: Option<&str>) -> Todo {
        Todo {
            guild_id: "1".into(),
            number,
            title: "Module Pizza Feature On Misty Bot".into(),
            details: String::new(),
            author_id: "2".into(),
            created_at: 1_800_000_000,
            claimed_by: claimed.map(str::to_owned),
            claimed_at: claimed.map(|_| 1_800_000_100),
            done: false,
            done_by: None,
            done_at: None,
        }
    }

    /// Every component, nested ones included (Discord allows 40).
    fn count(value: &serde_json::Value) -> usize {
        let children = |key: &str| {
            value[key]
                .as_array()
                .map_or(0, |list| list.iter().map(count).sum::<usize>())
        };
        let accessory = if value["accessory"].is_object() { 1 } else { 0 };
        1 + children("components") + accessory
    }

    #[test]
    fn panel_buttons() {
        let mut config = GuildConfig::default();
        config.todo.enabled = true;
        let open = vec![task(1, None), task(2, Some("7"))];
        let mut done = task(3, Some("7"));
        done.done = true;
        done.done_by = Some("7".into());
        done.done_at = Some(1_800_000_500);
        let json = panel(&config, &open, &[done]).payload().to_string();
        assert!(json.contains("todo:claim:1"));
        assert!(json.contains("todo:finish:2"));
        assert!(!json.contains("todo:claim:2"));
        assert!(json.contains("Claimed by <@7>"));
        assert!(json.contains("~~#3 Module Pizza"));
        assert!(json.contains(ADD));
        // Mentions in the panel never ping.
        assert!(json.contains("\"users\":[]"));
    }

    #[test]
    fn panel_stays_within_discord_limits() {
        let mut config = GuildConfig::default();
        config.todo.text = "Pick a task.".into();
        let open: Vec<Todo> = (1..=30).map(|n| task(n, None)).collect();
        let done: Vec<Todo> = (31..=33).map(|n| task(n, None)).collect();
        let payload = panel(&config, &open, &done).payload();
        let total: usize = payload["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(count)
            .sum();
        assert!(total <= 40, "{total} components");
        assert!(payload.to_string().contains("And 23 more"));
    }
}
