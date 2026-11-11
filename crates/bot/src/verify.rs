//! Rules panel and member verification: a button, optionally followed by a
//! captcha (an image in Discord, or a Discord Activity), that grants the
//! verified role.

use std::time::{Duration, Instant};

use dc_core::{guild::GuildConfig, registry, render};

use crate::{
    app::{App, Captcha, roles_of},
    interactions::{Ctx, modal_value},
    logs,
    ui::{self, Msg},
};

const CAPTCHA_LIFETIME: Duration = Duration::from_secs(300);
const MAX_ATTEMPTS: u32 = 3;

/// The rules panel posted in the rules channel.
pub fn panel(config: &GuildConfig) -> Msg {
    let rules = &config.rules;
    let mut children = vec![
        ui::text(format!("# {}", rules.title)),
        ui::text(rules.text.clone()),
    ];
    if rules.verification != "none" {
        children.push(ui::separator());
        children.push(ui::section_button(
            "**Accept the rules** to get access to the server.",
            ui::button(ui::STYLE_SUCCESS, "Verify", "verify:start"),
        ));
    }
    Msg::card(ui::WHITE, children)
}

/// The Activity needs the application's client secret on the panel's side;
/// without it members get the image captcha instead.
fn activity_configured(app: &App) -> bool {
    registry::read_env(&app.dir, "DISCORD_CLIENT_SECRET").is_some()
}

/// Click on "Verify".
pub async fn start(ctx: &Ctx) {
    let config = ctx.config();
    let rules = &config.rules;
    let Some(role) = rules.verified_role.as_deref().filter(|_| rules.enabled) else {
        let text = "Verification is not fully set up yet. Ask an administrator.";
        return ctx
            .reply(Msg::warning("Unavailable", text).ephemeral())
            .await;
    };
    if roles_of(&ctx.member).iter().any(|owned| owned == role) {
        let text = "You already have access to the server.";
        return ctx
            .reply(Msg::success("Already verified", text).ephemeral())
            .await;
    }
    match rules.verification.as_str() {
        "captcha" => image_captcha(ctx).await,
        "activity" => {
            if activity_configured(&ctx.app) {
                match ctx.launch_activity().await {
                    Ok(()) => return,
                    // Activities not enabled for this application, or an old
                    // Discord client: the interaction is still unanswered.
                    Err(error) => {
                        tracing::warn!(guild = %ctx.guild_id, %error, "launching the Activity failed; using the image captcha");
                    }
                }
            }
            image_captcha(ctx).await;
        }
        "web" => match web_link(ctx) {
            Some(url) => {
                let text = "Open the link below and solve the captcha. The link only works \
                            for you and expires in 10 minutes.";
                let open = ui::row(vec![ui::link_button("Open the verification page", &url)]);
                let msg = Msg::banner(dc_core::cards::Tone::Info, "Verification", text, vec![open]);
                ctx.reply(msg.ephemeral()).await;
            }
            None => image_captcha(ctx).await,
        },
        _ => grant(ctx).await,
    }
}

/// Personal link to the verification page of the panel, signed for this
/// member and server. `None` when the panel's address is unknown.
fn web_link(ctx: &Ctx) -> Option<String> {
    let origin = ctx.app.panel_origin()?;
    let value = dc_core::verify_link::sign(
        ctx.app.api.token(),
        &ctx.app.slug,
        &ctx.guild_id,
        ctx.user_id(),
        dc_core::now(),
    );
    Some(format!("{origin}/verify/{}?v={value}", ctx.app.slug))
}

/// Sends the captcha image with a button that opens the answer form.
async fn image_captcha(ctx: &Ctx) {
    ctx.defer(true).await;
    let code = render::captcha_code();
    let svg = render::captcha(&code);
    let image = tokio::task::spawn_blocking(move || render::png(&svg)).await;
    let Ok(Ok(png)) = image else {
        tracing::error!("rendering the captcha failed");
        return ctx
            .reply(Msg::error("The captcha could not be created. Try again."))
            .await;
    };
    {
        let mut captchas = ctx.app.captchas.lock().unwrap();
        captchas.retain(|_, captcha| captcha.expires > Instant::now());
        captchas.insert(
            (ctx.guild_id.clone(), ctx.user_id().to_owned()),
            Captcha {
                code,
                expires: Instant::now() + CAPTCHA_LIFETIME,
                attempts: 0,
            },
        );
    }
    let msg = Msg::card(
        ui::WHITE,
        vec![
            ui::text("### One last step\nType the characters shown in the image. Letters can be upper or lower case."),
            ui::gallery("attachment://captcha.png"),
            ui::row(vec![ui::button(ui::STYLE_PRIMARY, "Enter the code", "verify:enter")]),
            ui::text("-# The code expires in 5 minutes. Click Verify again for a new one."),
        ],
    )
    .file("captcha.png", png);
    ctx.reply(msg).await;
}

/// Click on "Enter the code": opens the input form.
pub async fn ask_code(ctx: &Ctx) {
    let inputs = [(
        "code",
        "Code shown in the image",
        "ABC12",
        render::CAPTCHA_LENGTH as u32,
    )];
    ctx.modal(ui::modal("verify:code", "Verification", &inputs))
        .await;
}

/// The member submitted the form.
pub async fn check_code(ctx: &Ctx) {
    let answer = modal_value(&ctx.data, "code").trim().to_uppercase();
    let key = (ctx.guild_id.clone(), ctx.user_id().to_owned());

    enum Outcome {
        Passed,
        Wrong(u32),
        Expired,
    }
    let outcome = {
        let mut captchas = ctx.app.captchas.lock().unwrap();
        match captchas.get_mut(&key) {
            Some(captcha) if captcha.expires > Instant::now() => {
                if captcha.code == answer {
                    captchas.remove(&key);
                    Outcome::Passed
                } else {
                    captcha.attempts += 1;
                    let left = MAX_ATTEMPTS.saturating_sub(captcha.attempts);
                    if left == 0 {
                        captchas.remove(&key);
                    }
                    Outcome::Wrong(left)
                }
            }
            _ => {
                captchas.remove(&key);
                Outcome::Expired
            }
        }
    };

    match outcome {
        Outcome::Passed => grant(ctx).await,
        Outcome::Wrong(0) | Outcome::Expired => {
            let text = "Click **Verify** again to get a new code.";
            ctx.reply(Msg::warning("Code expired", text).ephemeral())
                .await;
        }
        Outcome::Wrong(left) => {
            let text = format!("That is not the code in the image. {left} attempt(s) left.");
            ctx.reply(Msg::warning("Wrong code", &text).ephemeral())
                .await;
        }
    }
}

/// Why a member could not be verified.
#[derive(Debug)]
pub enum GrantError {
    NotSetUp,
    Discord(crate::api::ApiError),
}

/// Gives the verified role, takes the "unverified" one away and logs it.
/// Used by the button, the image captcha and the Activity.
pub async fn grant_roles(app: &App, guild_id: &str, user_id: &str) -> Result<(), GrantError> {
    let config = app.config(guild_id);
    let rules = &config.rules;
    let Some(role) = rules.verified_role.as_deref().filter(|_| rules.enabled) else {
        return Err(GrantError::NotSetUp);
    };
    if let Err(error) = app.api.add_role(guild_id, user_id, role, "Verified").await {
        tracing::warn!(guild = guild_id, %error, "giving the verified role failed");
        return Err(GrantError::Discord(error));
    }
    if let Some(old) = &rules.remove_role
        && let Err(error) = app
            .api
            .remove_role(guild_id, user_id, old, "Verified")
            .await
    {
        // The member is verified all the same; an admin can fix the role.
        tracing::warn!(guild = guild_id, %error, "removing the pre-verification role failed");
    }
    let entry = format!("<@{user_id}> passed the verification.");
    logs::send(
        app,
        guild_id,
        logs::Kind::Members,
        ui::GREEN,
        "Member verified",
        &entry,
    )
    .await;
    Ok(())
}

async fn grant(ctx: &Ctx) {
    let reply = match grant_roles(&ctx.app, &ctx.guild_id, ctx.user_id()).await {
        Ok(()) => {
            let text = "Welcome! You now have access to the server.";
            Msg::success("Verified", text).ephemeral()
        }
        Err(GrantError::NotSetUp) => Msg::error("Verification is not set up anymore."),
        Err(GrantError::Discord(_)) => Msg::error(
            "I could not give you the role. Ask an administrator to check my permissions.",
        ),
    };
    ctx.reply(reply).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_has_button_only_when_verifying() {
        let mut config = GuildConfig::default();
        config.rules.verification = "none".into();
        let plain = panel(&config).payload().to_string();
        assert!(!plain.contains("verify:start"));

        for mode in ["button", "captcha", "activity"] {
            config.rules.verification = mode.into();
            let with_button = panel(&config).payload().to_string();
            assert!(with_button.contains("verify:start"));
            assert!(with_button.contains("Server rules"));
        }
    }
}
