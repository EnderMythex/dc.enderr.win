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
pub const CLOSE_DMS_PNG: &[u8] = dc_core::tips::CLOSE_DMS_PNG;
pub const CLOSE_DMS_FILE: &str = "close-dms.png";

pub fn scam_tips(lang: Lang) -> (&'static str, &'static str) {
    dc_core::tips::scam(lang == Lang::Fr)
}

pub fn dm_tips(lang: Lang) -> (&'static str, &'static str) {
    dc_core::tips::dms(lang == Lang::Fr)
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
    let title = dc_core::tips::reminder_title(lang == Lang::Fr);
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
