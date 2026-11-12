//! A panel of buttons members click to take or drop roles. Posted and kept
//! up to date by `panels::sync`, like the rules and ticket panels.

use dc_core::guild::GuildConfig;

use crate::{
    app::roles_of,
    interactions::Ctx,
    moderation::may_be_self_assigned,
    ui::{self, Msg},
};

pub const PREFIX: &str = "role:";
/// Buttons of panels posted by the former Scanline bot, still answered.
pub const OLD_PREFIX: &str = "role-toggle:";

pub fn panel(config: &GuildConfig) -> Msg {
    let panel = &config.role_panel;
    let mut children = Vec::new();
    for (index, button) in panel.buttons.iter().enumerate() {
        if index > 0 {
            children.push(ui::separator());
        }
        let mut text = format!("**{}**", button.label);
        if !button.description.is_empty() {
            text.push_str(&format!("\n{}", button.description));
        }
        let id = format!("{PREFIX}{}", button.role);
        let control = if button.emoji.is_empty() {
            ui::button(ui::STYLE_SECONDARY, &button.label, &id)
        } else {
            ui::emoji_button(ui::STYLE_SECONDARY, &button.emoji, &button.label, &id)
        };
        children.push(ui::section_button(text, control));
    }
    children.push(ui::separator());
    children.push(ui::text("-# Click a button to take or drop the role."));
    ui::panel_card(&panel.look, &panel.title, children)
}

/// A button of the panel: toggles the role.
pub async fn toggle(ctx: &Ctx) {
    let id = ctx.custom_id();
    let role = id
        .strip_prefix(OLD_PREFIX)
        .or_else(|| id.strip_prefix(PREFIX))
        .unwrap_or_default()
        .to_owned();
    // Only roles of the current panel: an old or forged button gives nothing.
    let config = ctx.config();
    let offered =
        config.role_panel.enabled && config.role_panel.buttons.iter().any(|b| b.role == role);
    if !offered || !may_be_self_assigned(&ctx.app, &ctx.guild_id, &role) {
        return ctx
            .reply(Msg::error("This role is no longer offered here."))
            .await;
    }
    let has = roles_of(&ctx.member).contains(&role);
    let api = &ctx.app.api;
    let result = if has {
        api.remove_role(&ctx.guild_id, ctx.user_id(), &role, "Role panel")
            .await
    } else {
        api.add_role(&ctx.guild_id, ctx.user_id(), &role, "Role panel")
            .await
    };
    let msg = match result {
        Ok(_) if has => Msg::success("Role removed", &format!("You no longer have <@&{role}>.")),
        Ok(_) => Msg::success("Role added", &format!("You now have <@&{role}>.")),
        Err(error) => Msg::error(&error.user_message()),
    };
    ctx.reply(msg.ephemeral()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::guild::RoleButton;

    #[test]
    fn panel_has_one_button_per_role() {
        let mut config = GuildConfig::default();
        config.role_panel.buttons = vec![
            RoleButton {
                role: "11".into(),
                label: "FR".into(),
                emoji: "🇫🇷".into(),
                description: "Français".into(),
            },
            RoleButton {
                role: "12".into(),
                label: "EN".into(),
                emoji: String::new(),
                description: String::new(),
            },
        ];
        let payload = panel(&config).payload().to_string();
        assert!(payload.contains("role:11") && payload.contains("role:12"));
        assert!(payload.contains("Français"));
    }
}
