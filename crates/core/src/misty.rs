//! Import of the settings of the former Misty bot (its `data/state.json`)
//! into the sections of a server.
//!
//! Misty kept every server in one file: `{"guilds": {"<id>": {...}},
//! "cursors": {...}}`, with ids as numbers. Its defaults (repository,
//! website, rules, ticket categories) are filled in where the file left a
//! value empty, the way Misty itself did.

use serde_json::{Map, Value, json};

pub const REPO: &str = "Misty-MC/Misty";
pub const SITE: &str = "https://misty-launcher.online";

/// Is this the `state.json` of Misty?
pub fn is_state(value: &Value) -> bool {
    let Some(guilds) = value.get("guilds").and_then(Value::as_object) else {
        return false;
    };
    value.get("cursors").is_some()
        || guilds.values().any(|guild| {
            ["releases", "git", "verify", "smp"]
                .iter()
                .any(|key| guild.get(key).is_some())
        })
}

/// An id written as a number or a string.
fn id(value: &Value) -> Value {
    match value {
        Value::Number(n) => json!(n.to_string()),
        Value::String(s) if crate::is_snowflake(s) => json!(s),
        _ => Value::Null,
    }
}

fn ids(value: &Value) -> Value {
    match id(value) {
        Value::Null => json!([]),
        one => json!([one]),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

fn flag(value: &Value, key: &str, fallback: bool) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(fallback)
}

fn pick<'a>(french: bool, fr: &'a str, en: &'a str) -> &'a str {
    if french { fr } else { en }
}

fn default_rules(french: bool) -> &'static str {
    pick(
        french,
        "**1.** Respecte tout le monde : pas d'insultes, de harcèlement ni de discrimination.\n\
         **2.** Pas de spam, de publicité ni de liens douteux.\n\
         **3.** Utilise le bon salon pour chaque sujet.\n\
         **4.** Pas de contenu NSFW, illégal ou de triche.\n\
         **5.** Respecte les [conditions d'utilisation de Discord](https://discord.com/terms).",
        "**1.** Respect everyone: no insults, harassment or discrimination.\n\
         **2.** No spam, advertising or shady links.\n\
         **3.** Use the right channel for each topic.\n\
         **4.** No NSFW, illegal or cheating content.\n\
         **5.** Follow the [Discord Terms of Service](https://discord.com/terms).",
    )
}

fn default_welcome(french: bool) -> &'static str {
    pick(
        french,
        "Bienvenue {user} sur **{server}** ! Tu es notre {count}e membre, installe-toi et dis bonjour.",
        "Welcome {user} to **{server}**! You're member #{count}, make yourself at home and say hi.",
    )
}

fn default_topics(french: bool) -> Vec<&'static str> {
    if french {
        vec![
            "Aide avec le launcher",
            "Signaler un bug",
            "Signaler un membre",
            "Proposer une fonctionnalité",
            "Partenariat",
            "Autre",
        ]
    } else {
        vec![
            "Launcher help",
            "Report a bug",
            "Report a member",
            "Suggest a feature",
            "Partnership",
            "Other",
        ]
    }
}

/// The websites Misty's status panel watched by default.
fn default_sites(french: bool) -> Value {
    json!([
        { "name": "Site", "url": format!("{SITE}/") },
        { "name": "API", "url": format!("{SITE}/api/latest") },
        { "name": "Docs", "url": format!("{SITE}/docs") },
        { "name": pick(french, "Boutique", "Shop"), "url": format!("{SITE}/shop") },
    ])
}

/// The sections of one server of a Misty `state.json`, in this panel's
/// format, and the id of the Misty server they come from. `guild_id`
/// picks the server; a file with a single server is used whatever its id.
pub fn sections(state: &Value, guild_id: &str) -> Result<(Map<String, Value>, String), String> {
    let guilds = state
        .get("guilds")
        .and_then(Value::as_object)
        .ok_or("This Misty file has no servers in it.")?;
    let (source, g) = match guilds.get(guild_id) {
        Some(g) => (guild_id.to_owned(), g),
        None if guilds.len() == 1 => {
            let (id, g) = guilds.iter().next().unwrap();
            (id.clone(), g)
        }
        None => {
            let known: Vec<&str> = guilds.keys().map(String::as_str).collect();
            return Err(format!(
                "This Misty file has no settings for this server. Servers in the file: {}.",
                known.join(", ")
            ));
        }
    };
    let null = Value::Null;
    let part = |key: &str| g.get(key).unwrap_or(&null);
    // Misty spoke English unless French was chosen in /settings.
    let french = text(g, "lang") == "fr" && flag(g, "lang_chosen", false);

    let mut out = Map::new();

    let welcome = part("welcome");
    let message = match text(welcome, "message") {
        "" => default_welcome(french),
        custom => custom,
    };
    out.insert(
        "welcome".into(),
        json!({
            "enabled": flag(welcome, "enabled", false) && !id(&welcome["channel"]).is_null(),
            "channel": id(&welcome["channel"]),
            "message": message,
            "image": flag(welcome, "image", true),
            "auto_roles": ids(&welcome["role"]),
        }),
    );

    let verify = part("verify");
    let on = flag(verify, "enabled", false);
    let rules = match text(verify, "rules") {
        "" => default_rules(french),
        custom => custom,
    };
    // Misty's captcha ran on its website; here the panel's own page does it.
    let mode = if flag(verify, "captcha", true) {
        "web"
    } else {
        "button"
    };
    out.insert(
        "rules".into(),
        json!({
            "enabled": on,
            "channel": id(&verify["channel"]),
            "title": pick(french, "Règlement du serveur", "Server rules"),
            "text": rules,
            "verify_enabled": on,
            "verify_channel": null,
            "verify_title": pick(french, "Vérification", "Verification"),
            "verify_text": pick(french, "Clique sur le bouton pour accéder au serveur.", "Click the button to access the server."),
            "verification": mode,
            "look": {
                "color": "#6f9cff",
                "banner": true,
                "label": pick(french, "Bienvenue", "Welcome"),
                "subtitle": pick(french, "Lis les règles puis vérifie-toi pour accéder au serveur.", "Read the rules, then verify yourself to access the server."),
            },
            "verified_role": id(&verify["role"]),
            "remove_role": id(&verify["remove"]),
            "captcha_style": "text",
        }),
    );
    // Misty wrote each verification in a log channel.
    if !id(&verify["log"]).is_null() {
        out.insert(
            "logs".into(),
            json!({
                "enabled": true,
                "channel": id(&verify["log"]),
                "message_delete": false,
                "message_edit": false,
                "channels": false,
                "members": true,
                "moderation": false,
            }),
        );
    }

    let tickets = part("tickets");
    let mut topics: Vec<String> = tickets
        .get("kinds")
        .and_then(Value::as_array)
        .map(|kinds| {
            kinds
                .iter()
                .map(|kind| text(kind, "label").to_owned())
                .filter(|label| !label.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if topics.is_empty() {
        topics = default_topics(french)
            .into_iter()
            .map(str::to_owned)
            .collect();
    }
    out.insert(
        "tickets".into(),
        json!({
            "enabled": flag(tickets, "enabled", false),
            "panel_channel": id(&tickets["channel"]),
            "category": id(&tickets["category"]),
            "staff_roles": ids(&tickets["staff"]),
            "log_channel": id(&tickets["log"]),
            "title": pick(french, "Besoin d'aide ?", "Need help?"),
            "text": pick(
                french,
                "Ouvre un ticket pour parler au staff en privé. Choisis la catégorie qui correspond à ta demande, et un salon rien qu'à toi s'ouvre.",
                "Open a ticket to talk to the staff privately. Pick the category that matches your request, and a channel just for you opens.",
            ),
            "topics": topics,
            "max_open": 1,
            // Misty asked for the request in a form before opening.
            "questions": flag(tickets, "ask_reason", true),
            "look": {
                "color": "#6f9cff",
                "banner": true,
                "label": "Support",
                "subtitle": pick(french, "Choisis une catégorie : un salon privé s'ouvre avec le staff.", "Pick a category: a private channel opens with the staff."),
            },
        }),
    );

    let releases = part("releases");
    out.insert(
        "releases".into(),
        json!({
            "enabled": flag(releases, "enabled", false),
            "channel": id(&releases["channel"]),
            "role": id(&releases["role"]),
            "repo": REPO,
            "prereleases": flag(releases, "prereleases", false),
            "download_url": format!("{SITE}/download/setup"),
            "website": format!("{SITE}/"),
            "look": { "color": "#6f9cff" },
        }),
    );

    let git = part("git");
    let events = git
        .get("events")
        .cloned()
        .unwrap_or_else(|| json!(["push", "pull_request", "issues"]));
    out.insert(
        "github".into(),
        json!({
            "enabled": flag(git, "enabled", false),
            "channel": id(&git["channel"]),
            "repos": git.get("repos").cloned().unwrap_or_else(|| json!([])),
            "events": events,
        }),
    );

    let status = part("status");
    out.insert(
        "status".into(),
        json!({
            "enabled": flag(status, "enabled", false),
            "channel": id(&status["channel"]),
            "sites": default_sites(french),
            "repo": REPO,
            "players_url": "",
        }),
    );

    let smp = part("smp");
    let name = match text(smp, "name") {
        "" => "Misty SMP",
        custom => custom,
    };
    out.insert(
        "minecraft".into(),
        json!({
            "enabled": flag(smp, "enabled", false),
            "address": text(smp, "address"),
            "name": name,
            "description": text(smp, "description"),
            "show_players": flag(smp, "show_players", true),
            "channel": id(&smp["channel"]),
        }),
    );

    Ok((out, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guild::GuildConfig;

    const STATE: &str = r#"{
      "guilds": {
        "1474887049726267434": {
          "lang": "fr", "lang_chosen": true,
          "releases": { "enabled": true, "channel": 1474887049726267001, "role": 1474887049726267002, "prereleases": false },
          "git": { "enabled": true, "channel": 1474887049726267003, "repos": ["Misty-MC/Misty", "Misty-MC/Misty-Web"], "events": ["push", "star"] },
          "status": { "enabled": true, "channel": 1474887049726267004, "message": 1 },
          "verify": { "enabled": true, "channel": 1474887049726267005, "role": 1474887049726267006, "remove": null, "captcha": true, "activity": true, "log": 1474887049726267007, "rules": "" },
          "welcome": { "enabled": true, "channel": 1474887049726267008, "role": 1474887049726267009, "image": true, "message": "" },
          "tickets": { "enabled": true, "channel": 1474887049726267010, "category": 1474887049726267011, "staff": 1474887049726267012, "log": 1474887049726267013, "kinds": [{"label": "Bug", "description": "x"}], "open": {"1": {}} },
          "smp": { "enabled": true, "address": "play.misty.fr", "name": "", "description": "Survie", "show_players": true, "channel": 1474887049726267014 }
        }
      },
      "cursors": { "releases": {} }
    }"#;

    #[test]
    fn converts_a_misty_server() {
        let state: Value = serde_json::from_str(STATE).unwrap();
        assert!(is_state(&state));
        assert!(!is_state(&json!({ "settings": {} })));
        // Another id: the only server of the file is used.
        let (sections, source) = sections(&state, "175928847299117063").unwrap();
        assert_eq!(source, "1474887049726267434");

        let mut merged = serde_json::to_value(GuildConfig::default()).unwrap();
        for (key, value) in sections {
            merged[&key] = value;
        }
        let mut config: GuildConfig = serde_json::from_value(merged).unwrap();
        config.sanitize();
        assert!(config.releases.enabled && config.releases.repo == REPO);
        assert_eq!(config.releases.role.as_deref(), Some("1474887049726267002"));
        assert_eq!(config.github.repos.len(), 2);
        assert_eq!(config.github.events, vec!["push", "star"]);
        assert_eq!(config.status.sites.len(), 4);
        assert!(config.rules.verifying() && config.rules.verification == "web");
        assert!(config.rules.text.contains("Respecte"));
        assert_eq!(config.welcome.auto_roles, vec!["1474887049726267009"]);
        assert_eq!(config.tickets.topics, vec!["Bug"]);
        assert!(config.tickets.questions);
        assert!(config.rules.look.banner && config.rules.look.color == "#6f9cff");
        assert_eq!(config.releases.look.color_or(0), 0x6F9CFF);
        assert_eq!(config.tickets.staff_roles, vec!["1474887049726267012"]);
        assert!(config.logs.enabled && config.logs.members && !config.logs.message_delete);
        assert_eq!(config.minecraft.name, "Misty SMP");
        assert_eq!(config.minecraft.address, "play.misty.fr");
    }

    #[test]
    fn several_servers_need_the_right_one() {
        let state = json!({ "guilds": { "1": { "verify": {} }, "2": { "verify": {} } } });
        assert!(sections(&state, "3").unwrap_err().contains("1, 2"));
        assert!(sections(&state, "2").is_ok());
    }
}
