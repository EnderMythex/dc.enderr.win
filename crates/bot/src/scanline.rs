//! The Scanline pack: `/filters`, `/tools`, `/tuto-terminal` and
//! `/wallet-tracker`, sending the files of the bot's `assets/` folder.
//! Registered only for bots whose `bot.toml` lists `packs = ["scanline"]`.

use std::{path::PathBuf, sync::LazyLock};

use regex::Regex;

use crate::{
    app::App,
    interactions::Ctx,
    lang::Lang,
    ui::{self, Msg},
};

pub const PACK: &str = dc_core::commands::SCANLINE;
pub const FILTER_PREFIX: &str = "filters:";

fn asset(app: &App, name: &str) -> PathBuf {
    app.dir.join(dc_core::registry::ASSETS_DIR).join(name)
}

fn read(app: &App, name: &str) -> Option<Vec<u8>> {
    std::fs::read(asset(app, name))
        .map_err(|error| tracing::warn!(file = name, %error, "missing pack file"))
        .ok()
}

/// The AI's default knowledge for this bot (`assets/assistant.md`).
pub fn default_context(app: &App) -> String {
    std::fs::read_to_string(asset(app, "assistant.md")).unwrap_or_default()
}

static FILTER_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(filtres?|filters?|config(uration)?s?|presets?|setups?)\b").unwrap()
});
static WALLET_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)wallets?\b").unwrap());
static TRACK_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)track(?:er|ing)?s?\b|traqu(?:er|eur|é)|suivi").unwrap());

/// Cheap first filter before asking the AI what the member wants.
pub fn may_want_files(question: &str) -> bool {
    FILTER_WORDS.is_match(question)
        || (WALLET_WORD.is_match(question) && TRACK_WORD.is_match(question))
}

/// A card with a text and files: pictures shown, other files attached.
fn with_files(app: &App, text: &str, files: &[(String, String)]) -> Msg {
    let mut children = vec![ui::text(text)];
    let mut attached = Vec::new();
    for (path, name) in files {
        let Some(bytes) = read(app, path) else {
            continue;
        };
        children.push(if name.ends_with(".png") {
            ui::gallery(&format!("attachment://{name}"))
        } else {
            ui::file(&format!("attachment://{name}"))
        });
        attached.push((name.clone(), bytes));
    }
    let mut msg = Msg::card(ui::WHITE, children);
    for (name, bytes) in attached {
        msg = msg.file(&name, bytes);
    }
    msg
}

pub fn filters_prompt(lang: Lang) -> Msg {
    let buttons = ui::row(vec![
        ui::button(
            ui::STYLE_PRIMARY,
            "Axiom (Solana)",
            &format!("{FILTER_PREFIX}axiom"),
        ),
        ui::button(
            ui::STYLE_PRIMARY,
            "Axiom (Robinhood)",
            &format!("{FILTER_PREFIX}robinhood"),
        ),
        ui::button(
            ui::STYLE_SECONDARY,
            "Terminal",
            &format!("{FILTER_PREFIX}terminal"),
        ),
    ]);
    Msg::card(
        ui::WHITE,
        vec![
            ui::text(lang.pick(
                "### Filtres\nTu veux mes filtres pour quelle plateforme ?",
                "### Filters\nWhich platform do you want my filters for?",
            )),
            buttons,
        ],
    )
}

pub fn filters(app: &App, choice: &str, lang: Lang) -> Msg {
    let set = |folder: &str, prefix: &str| -> Vec<(String, String)> {
        ["newpairs.json", "finalStretch.json", "migrated.json"]
            .into_iter()
            .map(|name| (format!("filters/{folder}{name}"), format!("{prefix}{name}")))
            .collect()
    };
    let guide = (
        "how-to-import-filters.png".to_owned(),
        "how-to-import-filters.png".to_owned(),
    );
    match choice {
        "terminal" => with_files(
            app,
            lang.pick(
                "Voici mes filtres Terminal (Padre) — New Pairs / Final Stretch / Migrated — en fichiers à importer.",
                "Here are my Terminal (Padre) filters — New Pairs / Final Stretch / Migrated — as importable files.",
            ),
            &set("terminal/", ""),
        ),
        // Same file names as the Solana set: prefixed so both can be imported.
        "robinhood" => {
            let mut files = set("robinhood/", "robinhood-");
            files.push(guide);
            with_files(
                app,
                lang.pick(
                    "Voici mes filtres Axiom pour la chaîne Robinhood (New Pairs / Final Stretch / Migrated). Filtres > Import, puis tu sélectionnes le fichier voulu :",
                    "Here are my Axiom filters for the Robinhood chain (New Pairs / Final Stretch / Migrated). Filters > Import, then pick the file you want:",
                ),
                &files,
            )
        }
        _ => {
            let mut files = set("", "");
            files.push(guide);
            with_files(
                app,
                lang.pick(
                    "Voici mes filtres Axiom (New Pairs / Final Stretch / Migrated) en fichiers à importer. Filtres > Import, puis tu sélectionnes le fichier voulu :",
                    "Here are my Axiom filters (New Pairs / Final Stretch / Migrated) as importable files. Filters > Import, then pick the file you want:",
                ),
                &files,
            )
        }
    }
}

pub fn wallet_tracker(app: &App, lang: Lang) -> Msg {
    with_files(
        app,
        lang.pick(
            "Voici mon wallet tracker à importer sur Axiom. Wallet Tracker > Import, puis tu sélectionnes le fichier :",
            "Here's my wallet tracker to import on Axiom. Wallet Tracker > Import, then pick the file:",
        ),
        &[
            ("wallet-tracker.json".to_owned(), "wallet-tracker.json".to_owned()),
            ("how-to-import-wallets.png".to_owned(), "how-to-import-wallets.png".to_owned()),
        ],
    )
}

fn tools(app: &App, lang: Lang) -> Msg {
    let file = lang.pick("tools-fr.md", "tools-en.md");
    let text = std::fs::read_to_string(asset(app, file)).unwrap_or_else(|_| "—".into());
    Msg::card(ui::WHITE, vec![ui::text(text)])
}

fn tuto_terminal(app: &App, lang: Lang) -> Msg {
    with_files(
        app,
        lang.pick(
            "📄 Voici le tuto pour configurer Terminal (Padre) : thème, couleurs des marqueurs, colonne Trenches, import du wallet et presets de frais.",
            "📄 Here's the tutorial to set up Terminal (Padre): theme, chart marker colors, Trenches column, wallet import and fee presets.",
        ),
        &[("tuto-terminal.pdf".to_owned(), "tuto-terminal.pdf".to_owned())],
    )
}

/// `/filters`, `/tools`, `/tuto-terminal`, `/wallet-tracker`: open to everyone.
pub async fn command(ctx: &Ctx, name: &str) {
    let lang = ctx.lang();
    let msg = match name {
        "filters" => filters_prompt(lang),
        "tools" => tools(&ctx.app, lang),
        "tuto-terminal" => {
            ctx.defer(true).await;
            tuto_terminal(&ctx.app, lang)
        }
        _ => wallet_tracker(&ctx.app, lang),
    };
    ctx.reply(msg.ephemeral()).await;
}

/// A button of the `/filters` choice.
pub async fn filter_choice(ctx: &Ctx) {
    let choice = ctx.custom_id().trim_start_matches(FILTER_PREFIX).to_owned();
    ctx.defer(true).await;
    ctx.reply(filters(&ctx.app, &choice, ctx.lang()).ephemeral())
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_that_may_want_files() {
        assert!(may_want_files("tu peux me donner tes filtres ?"));
        assert!(may_want_files("what's your config"));
        assert!(may_want_files("t'as une liste de wallets à tracker ?"));
        assert!(!may_want_files("my wallet got drained"));
        assert!(!may_want_files("salut ça va"));
    }
}
