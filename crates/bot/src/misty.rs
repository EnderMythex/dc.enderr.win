//! Commands of the `misty` pack (ported from the Misty bot): `/status`,
//! `/release`, `/download` and `/smp`, answered privately, and the Refresh
//! buttons of the status and Minecraft panels.

use crate::{ghfeed, github, interactions::Ctx, minecraft, ui::Msg, uptime};

pub const PACK: &str = dc_core::commands::MISTY;

/// Misty became Mistly: the saved settings of every server point to the new
/// website (links, status panel, download address). Runs at startup; does
/// nothing once done.
pub fn move_to_new_site(bot_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(bot_dir.join(dc_core::guild::DIR_NAME)) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(updated) = dc_core::misty::with_new_site(&raw) else {
            continue;
        };
        let tmp = path.with_extension("json.site.tmp");
        let written = std::fs::write(&tmp, updated).and_then(|()| std::fs::rename(&tmp, &path));
        match written {
            Ok(()) => {
                tracing::info!(file = %path.display(), "settings moved to {}", dc_core::misty::HOST)
            }
            Err(error) => {
                tracing::warn!(%error, file = %path.display(), "settings not moved to the new website")
            }
        }
    }
}

pub async fn command(ctx: &Ctx, name: &str) {
    ctx.defer(true).await;
    let config = ctx.config();
    let not_set = |what: &str| {
        Msg::warning(
            "Not set up",
            &format!(
                "{what} is not set up on this server yet. An administrator can do it on the dashboard."
            ),
        )
    };
    let msg = match name {
        "status" => {
            if config.status.sites.is_empty() && config.status.repo_or(&config.releases).is_empty()
            {
                not_set("The status panel")
            } else {
                let snapshot = uptime::measure(&ctx.app, &config, false).await;
                uptime::card(&ctx.app, &config, &snapshot)
            }
        }
        "release" | "download" => match ghfeed::release_repo(&config.releases) {
            None => not_set("The release announcements"),
            Some(repo) => {
                let releases = ctx.app.github.releases(repo).await.unwrap_or_default();
                let latest = github::latest(&releases);
                match (name, latest) {
                    ("download", _) => ghfeed::download_card(&config.releases, repo, latest),
                    (_, Some(release)) => {
                        ghfeed::release_card(&config.releases, repo, release, false)
                    }
                    (_, None) => Msg::warning(
                        "No version yet",
                        "GitHub cannot be reached right now, or there is no release yet.",
                    ),
                }
            }
        },
        _ => {
            let settings = &config.minecraft;
            if settings.address.is_empty() || !settings.enabled {
                not_set("The Minecraft server")
            } else {
                let status = ctx.app.pinger.status(&ctx.app.web, &settings.address).await;
                minecraft::card(settings, &status)
            }
        }
    };
    ctx.reply(msg).await;
}

/// Refresh button of a status or Minecraft card (a panel or a private
/// answer): the card is redrawn in place.
pub async fn refresh(ctx: &Ctx) {
    ctx.defer_update().await;
    let config = ctx.config();
    let msg = if ctx.custom_id() == uptime::REFRESH {
        let snapshot = uptime::measure(&ctx.app, &config, false).await;
        uptime::card(&ctx.app, &config, &snapshot)
    } else {
        let status = ctx
            .app
            .pinger
            .status(&ctx.app.web, &config.minecraft.address)
            .await;
        minecraft::card(&config.minecraft, &status)
    };
    ctx.reply(msg).await;
}
