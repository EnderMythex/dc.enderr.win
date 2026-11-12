//! The patch notes of the panel and its bots, newest first, shown at
//! `/patch-notes`. Add an entry at the top for each update.

pub struct Entry {
    /// Unique, used to show a dot on the button until the notes are read.
    pub id: &'static str,
    pub date: &'static str,
    pub title: &'static str,
    pub items: &'static [&'static str],
}

pub const ENTRIES: &[Entry] = &[
    Entry {
        id: "2026-11-12-todo",
        date: "2026-11-12",
        title: "Staff tasks on Misty Bot",
        items: &[
            "New section Staff tasks: a panel with each task to do and a button on its right, Claim, then Finish once claimed.",
            "The staff adds tasks with the Add a task button or /todo add (optionally claimed for someone), and uses /todo claim, unclaim, finish, remove and list.",
            "Only the member who claimed a task, or a bot admin, can finish it or give it back. The last finished tasks show crossed out.",
        ],
    },
    Entry {
        id: "2026-11-12-captcha-commands",
        date: "2026-11-12",
        title: "Your own captcha picture, commands on and off",
        items: &[
            "Slide captcha: choose its picture in Settings (or reuse the card background); each captcha zooms and moves it at random.",
            "Commands tab: a switch per command. A command turned off disappears from Discord on every server within a few seconds.",
        ],
    },
    Entry {
        id: "2026-11-12-light-theme",
        date: "2026-11-12",
        title: "Light theme, patch notes, choose what an import changes",
        items: &[
            "Light theme for the whole site: the button at the top (or in the footer) switches it; without a choice the site follows your system.",
            "Patch notes: this page, with a dot on the button when something new came out.",
            "Import review: tick or untick each change before confirming, or a whole section at once.",
        ],
    },
    Entry {
        id: "2026-11-12-looks",
        date: "2026-11-12",
        title: "Panels that look like Misty",
        items: &[
            "Rules, verification, tickets and role panels: custom side color and an optional banner (title on the card background, small label, subtitle).",
            "Release announcements in the Misty style: version and date on a banner, the role mentioned above the card, custom color.",
            "GitHub activity: custom side color.",
            "Import review: every change listed (before → after, with channel and role names), saved only once confirmed.",
            "Dashboard: the bots in a list, one line each.",
        ],
    },
    Entry {
        id: "2026-11-12-misty",
        date: "2026-11-12",
        title: "Misty Bot",
        items: &[
            "New bot: Misty Bot, with the night landscape of the Misty website behind its cards.",
            "Release announcements, live GitHub activity (commits, pull requests, issues, stars, forks, tags), a status panel with the history of each website, and a Minecraft server panel.",
            "Commands /status, /release, /download and /smp.",
            "Activity \"1,234 players · 0.3.7\" like the former Misty bot.",
            "Tickets: an optional form before opening, with questions chosen from the topic (bug, idea or other), in French or English.",
            "The former bot's data/state.json can be imported as is.",
        ],
    },
    Entry {
        id: "2026-11-12-backup",
        date: "2026-11-12",
        title: "Backup, new navigation, separate verification",
        items: &[
            "Backup: export the settings of a server as a file and import them on any server or bot.",
            "The sections of a server in a grouped side menu, with a dot on the ones in use.",
            "Rules and verification separated: a verification message in its own channel, or the verification without rules.",
            "Previews of the welcome DM, role panel, safety reminders and Twitch alerts.",
            "Commands tab, card background picture, panel activity log.",
            "Fewer Discord errors when browsing quickly; links of the AI assistant are clickable.",
        ],
    },
    Entry {
        id: "2026-11-11-scanline",
        date: "2026-11-11",
        title: "Scanline",
        items: &[
            "New bot: Scanline, with every Sang feature.",
            "Anti-scam (invites, \"insider\" pitch, scam pictures checked by AI), link review by the staff, role panel, AI assistant, Twitch alerts, safety reminders, welcome DM, color verification, SOL price activity.",
        ],
    },
    Entry {
        id: "2026-11-11-elyzium",
        date: "2026-11-11",
        title: "ElyZium Bot, announcements, security",
        items: &[
            "New bot: ElyZium Bot.",
            "Announcements and maintenance notices, giveaways, a role for every member, mentions of new members.",
            "Captcha on the website, legal pages, rate limits and security hardening.",
            "Unsaved changes bar and a reconnect screen during updates.",
        ],
    },
    Entry {
        id: "2026-11-11-first",
        date: "2026-11-11",
        title: "First version",
        items: &[
            "Log in with Discord and manage the bots assigned to you: start, stop, logs, profile and presence.",
            "Sang: moderation, levels, welcome, rules and verification, tickets, logs and settings from the dashboard.",
        ],
    },
];

/// Id of the newest entry.
pub fn latest() -> &'static str {
    ENTRIES.first().map_or("", |entry| entry.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_unique_and_newest_first() {
        let mut ids: Vec<&str> = ENTRIES.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ENTRIES.len());
        assert!(ENTRIES.windows(2).all(|w| w[0].date >= w[1].date));
        assert!(!latest().is_empty());
    }
}
