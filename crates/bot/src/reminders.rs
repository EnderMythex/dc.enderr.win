//! Safety reminders about scams, posted regularly in French and English
//! (ported from the Scanline bot), and the same tips in the welcome DM.

use std::{sync::Arc, time::Duration};

use dc_core::cards::Tone;

use crate::{
    app::App,
    db,
    lang::Lang,
    ui::{self, Msg},
};

const TICK: Duration = Duration::from_secs(600);
/// Picture showing where to turn off direct messages from server members.
pub const CLOSE_DMS_PNG: &[u8] = include_bytes!("../assets/images/close-dm.png");
pub const CLOSE_DMS_FILE: &str = "close-dms.png";

pub fn scam_tips(lang: Lang) -> (&'static str, &'static str) {
    lang.pick(
        (
            "⚠️ Attention aux scams",
            "Le staff ne t'enverra jamais de DM en premier et ne te demandera jamais ta seed phrase ou ta clé privée. Ne clique pas sur des liens reçus en DM (faux airdrops, wallet drainers) et vérifie toujours l'URL avant de connecter ton wallet. Si quelqu'un se fait passer pour le staff en privé, c'est un scam, signale-le.",
        ),
        (
            "⚠️ Beware of scams",
            "Staff will never DM you first and will never ask for your seed phrase or private key. Don't click links sent in DMs (fake airdrops, wallet drainers) and always check the URL before connecting your wallet. If someone impersonates staff in your DMs, it's a scam, report it.",
        ),
    )
}

pub fn dm_tips(lang: Lang) -> (&'static str, &'static str) {
    lang.pick(
        (
            "🔒 Ferme tes DMs",
            "Paramètres de confidentialité du serveur > désactive \"Autoriser les messages privés des membres du serveur\".",
        ),
        (
            "🔒 Turn off your DMs",
            "Server Privacy Settings > turn off \"Allow direct messages from server members\".",
        ),
    )
}

/// The tips as card blocks, with the picture (to attach as `CLOSE_DMS_FILE`).
pub fn tips_blocks(lang: Lang) -> Vec<serde_json::Value> {
    let (scam_title, scam_text) = scam_tips(lang);
    let (dm_title, dm_text) = dm_tips(lang);
    vec![
        ui::text(format!("**{scam_title}**\n{scam_text}")),
        ui::separator(),
        ui::text(format!("**{dm_title}**\n{dm_text}")),
        ui::gallery(&format!("attachment://{CLOSE_DMS_FILE}")),
    ]
}

pub fn reminder(lang: Lang) -> Msg {
    let title = lang.pick("Rappel de sécurité", "Safety reminder");
    Msg::banner(Tone::Warning, title, "", tips_blocks(lang))
        .file(CLOSE_DMS_FILE, CLOSE_DMS_PNG.to_vec())
}

/// Background task: posts the reminders of every server when they are due.
pub async fn watch(app: Arc<App>) {
    loop {
        tokio::time::sleep(TICK).await;
        let guilds: Vec<String> = app.guilds.lock().unwrap().keys().cloned().collect();
        for guild in guilds {
            let config = app.config(&guild);
            let settings = &config.reminders;
            if !settings.enabled || (settings.channel_fr.is_none() && settings.channel_en.is_none())
            {
                continue;
            }
            let last = db::get_counter(&app.db, &guild, "reminder")
                .await
                .unwrap_or(0);
            let now = dc_core::now();
            if now - last < i64::from(settings.interval_hours) * 3600 {
                continue;
            }
            for (channel, lang) in [
                (&settings.channel_fr, Lang::Fr),
                (&settings.channel_en, Lang::En),
            ] {
                if let Some(channel) = channel
                    && let Err(error) = app.api.send_message(channel, &reminder(lang)).await
                {
                    tracing::warn!(guild, %error, "posting a safety reminder failed");
                }
            }
            if let Err(error) = db::set_counter(&app.db, &guild, "reminder", now).await {
                tracing::error!(%error, "saving the reminder time failed");
            }
        }
    }
}
