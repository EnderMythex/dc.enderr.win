//! Slash command definitions, registered with Discord at startup.

use serde_json::{Value, json};

const SUB: u8 = 1;
const STRING: u8 = 3;
const INTEGER: u8 = 4;
const BOOLEAN: u8 = 5;
const USER: u8 = 6;
const CHANNEL: u8 = 7;
const ROLE: u8 = 8;

/// Channel types offered by channel pickers.
const TEXT_CHANNELS: [u8; 2] = [0, 5];
const CATEGORIES: [u8; 1] = [4];

fn option(kind: u8, name: &str, description: &str, required: bool) -> Value {
    json!({ "type": kind, "name": name, "description": description, "required": required })
}

fn channel(name: &str, description: &str, types: &[u8]) -> Value {
    let mut value = option(CHANNEL, name, description, false);
    value["channel_types"] = json!(types);
    value
}

fn sub(name: &str, description: &str, options: Vec<Value>) -> Value {
    json!({ "type": SUB, "name": name, "description": description, "options": options })
}

fn command(name: &str, description: &str, options: Vec<Value>) -> Value {
    json!({
        "name": name,
        "description": description,
        "options": options,
        // Server only: none of these make sense in private messages.
        "contexts": [0],
    })
}

fn user_and_reason(what: &str) -> Vec<Value> {
    vec![
        option(USER, "user", what, true),
        option(STRING, "reason", "Reason, shown in the audit log", false),
    ]
}

pub fn all() -> Vec<Value> {
    let target_channel = || channel("channel", "Channel (defaults to this one)", &TEXT_CHANNELS);
    let enabled = || option(BOOLEAN, "enabled", "Turn the module on or off", false);

    vec![
        command(
            "lock",
            "Prevent members from writing in a channel",
            vec![
                target_channel(),
                option(STRING, "reason", "Reason, shown in the channel", false),
            ],
        ),
        command(
            "unlock",
            "Let members write in a channel again",
            vec![target_channel()],
        ),
        command(
            "hide",
            "Hide a channel from members",
            vec![target_channel()],
        ),
        command(
            "unhide",
            "Make a hidden channel visible again",
            vec![target_channel()],
        ),
        command(
            "lockurl",
            "Block or allow links posted by members",
            vec![option(BOOLEAN, "enabled", "Block links?", true)],
        ),
        command(
            "ownerbot",
            "Manage who can administrate the bot on this server",
            vec![
                sub(
                    "add",
                    "Give a member access to the bot's admin commands",
                    vec![option(USER, "user", "Member to add", true)],
                ),
                sub(
                    "remove",
                    "Remove a member's admin access",
                    vec![option(USER, "user", "Member to remove", true)],
                ),
                sub("list", "Show the bot admins of this server", vec![]),
            ],
        ),
        command(
            "mute",
            "Time out a member",
            vec![
                option(USER, "user", "Member to mute", true),
                option(
                    STRING,
                    "duration",
                    "For how long: 10m, 2h, 1d… (28 days max)",
                    true,
                ),
                option(STRING, "reason", "Reason, shown in the audit log", false),
            ],
        ),
        command(
            "unmute",
            "Remove a member's timeout",
            vec![option(USER, "user", "Member to unmute", true)],
        ),
        command("ban", "Ban a member", {
            let mut options = user_and_reason("Member to ban");
            let mut days = option(
                INTEGER,
                "delete_days",
                "Delete their messages of the last N days (0-7)",
                false,
            );
            days["min_value"] = json!(0);
            days["max_value"] = json!(7);
            options.push(days);
            options
        }),
        command(
            "unban",
            "Lift a ban",
            vec![option(STRING, "user_id", "Id of the banned user", true)],
        ),
        command("kick", "Kick a member", user_and_reason("Member to kick")),
        command(
            "role",
            "Give or take a role",
            vec![
                sub(
                    "add",
                    "Give a role to a member",
                    vec![
                        option(USER, "user", "Member", true),
                        option(ROLE, "role", "Role to give", true),
                    ],
                ),
                sub(
                    "remove",
                    "Take a role from a member",
                    vec![
                        option(USER, "user", "Member", true),
                        option(ROLE, "role", "Role to take", true),
                    ],
                ),
                sub(
                    "add-all",
                    "Give a role to every member of the server (@everyone)",
                    vec![
                        option(ROLE, "role", "Role to give", true),
                        option(BOOLEAN, "include_bots", "Give it to bots too", false),
                    ],
                ),
                sub(
                    "remove-all",
                    "Take a role from every member of the server (@everyone)",
                    vec![
                        option(ROLE, "role", "Role to take", true),
                        option(BOOLEAN, "include_bots", "Take it from bots too", false),
                    ],
                ),
            ],
        ),
        command(
            "rank",
            "Show your level, or another member's",
            vec![option(USER, "user", "Member (defaults to you)", false)],
        ),
        command("leaderboard", "Show the most active members", vec![]),
        command(
            "xp",
            "Adjust a member's XP",
            vec![
                sub(
                    "add",
                    "Give XP to a member",
                    vec![
                        option(USER, "user", "Member", true),
                        option(INTEGER, "amount", "XP to give", true),
                    ],
                ),
                sub(
                    "remove",
                    "Take XP from a member",
                    vec![
                        option(USER, "user", "Member", true),
                        option(INTEGER, "amount", "XP to take", true),
                    ],
                ),
                sub(
                    "reset",
                    "Reset a member's XP to zero",
                    vec![option(USER, "user", "Member", true)],
                ),
            ],
        ),
        command(
            "settings",
            "Configure the bot on this server",
            vec![
                sub("view", "Show the current configuration", vec![]),
                sub(
                    "welcome",
                    "Welcome messages",
                    vec![
                        enabled(),
                        channel("channel", "Where to welcome new members", &TEXT_CHANNELS),
                        option(
                            STRING,
                            "message",
                            "Text; placeholders: {user} {name} {server} {count}",
                            false,
                        ),
                        option(BOOLEAN, "image", "Attach the welcome image?", false),
                        option(ROLE, "role", "Role given on arrival", false),
                    ],
                ),
                sub(
                    "rules",
                    "Rules panel and verification",
                    vec![
                        enabled(),
                        channel("channel", "Where the rules panel is posted", &TEXT_CHANNELS),
                        {
                            let mut mode =
                                option(STRING, "verification", "How members verify", false);
                            mode["choices"] = json!([
                                { "name": "None (rules only)", "value": "none" },
                                { "name": "Button", "value": "button" },
                                { "name": "Captcha image", "value": "captcha" },
                                { "name": "Captcha in a Discord Activity", "value": "activity" },
                            ]);
                            mode
                        },
                        option(ROLE, "role", "Role given once verified", false),
                    ],
                ),
                sub(
                    "tickets",
                    "Ticket system",
                    vec![
                        enabled(),
                        channel(
                            "channel",
                            "Where the ticket panel is posted",
                            &TEXT_CHANNELS,
                        ),
                        channel("category", "Category for ticket channels", &CATEGORIES),
                        option(
                            ROLE,
                            "staff_role",
                            "Role that can see and answer tickets",
                            false,
                        ),
                        channel("log_channel", "Where transcripts are sent", &TEXT_CHANNELS),
                    ],
                ),
                sub(
                    "xp",
                    "Levels and XP",
                    vec![
                        enabled(),
                        channel(
                            "announce_channel",
                            "Announce level-ups in this channel",
                            &TEXT_CHANNELS,
                        ),
                    ],
                ),
                sub(
                    "logs",
                    "Server logs",
                    vec![
                        enabled(),
                        channel("channel", "Where logs are sent", &TEXT_CHANNELS),
                    ],
                ),
                sub(
                    "preview",
                    "Send yourself a preview of the welcome message",
                    vec![],
                ),
            ],
        ),
        command(
            "giveaway",
            "Run giveaways",
            vec![
                sub(
                    "start",
                    "Start a giveaway",
                    vec![
                        option(STRING, "prize", "What the winners get", true),
                        option(
                            STRING,
                            "duration",
                            "How long it runs: 30m, 2h, 3d… (30 days max)",
                            true,
                        ),
                        {
                            let mut winners = option(
                                INTEGER,
                                "winners",
                                "Number of winners (1 by default)",
                                false,
                            );
                            winners["min_value"] = json!(1);
                            winners["max_value"] = json!(20);
                            winners
                        },
                        channel(
                            "channel",
                            "Where to post it (defaults to this channel)",
                            &TEXT_CHANNELS,
                        ),
                        option(
                            ROLE,
                            "required_role",
                            "Only members with this role can enter",
                            false,
                        ),
                    ],
                ),
                sub(
                    "end",
                    "End a giveaway now and draw its winners",
                    vec![option(
                        INTEGER,
                        "id",
                        "Giveaway number, e.g. 3 for #3",
                        true,
                    )],
                ),
                sub(
                    "reroll",
                    "Draw new winners for an ended giveaway",
                    vec![
                        option(INTEGER, "id", "Giveaway number, e.g. 3 for #3", true),
                        {
                            let mut winners = option(
                                INTEGER,
                                "winners",
                                "How many new winners (1 by default)",
                                false,
                            );
                            winners["min_value"] = json!(1);
                            winners["max_value"] = json!(20);
                            winners
                        },
                    ],
                ),
                sub(
                    "cancel",
                    "Cancel a giveaway without winners",
                    vec![option(
                        INTEGER,
                        "id",
                        "Giveaway number, e.g. 3 for #3",
                        true,
                    )],
                ),
                sub("list", "Show the running giveaways", vec![]),
            ],
        ),
        command("help", "List the bot's commands", vec![]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discord rejects the whole registration if a single command is invalid.
    #[test]
    fn definitions_respect_discord_limits() {
        fn check(options: &[Value]) {
            assert!(options.len() <= 25);
            let mut seen_optional = false;
            for option in options {
                let name = option["name"].as_str().unwrap();
                assert!(name.len() <= 32 && name == name.to_lowercase(), "{name}");
                assert!(!name.contains(' '), "{name}");
                let description = option["description"].as_str().unwrap();
                assert!(
                    (1..=100).contains(&description.chars().count()),
                    "{description}"
                );
                if option["type"] == SUB {
                    check(option["options"].as_array().unwrap());
                } else {
                    // Required options must come first.
                    let required = option["required"].as_bool().unwrap();
                    assert!(!(required && seen_optional), "{name}");
                    seen_optional |= !required;
                }
            }
        }
        let commands = all();
        let mut names: Vec<&str> = commands
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        for command in &commands {
            check(command["options"].as_array().unwrap());
        }
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate command name");
    }
}
