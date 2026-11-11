//! Legal pages, linked from the footer of every page: legal notice, privacy
//! policy and terms of use. The operator's identity comes from the
//! `LEGAL_NAME`, `LEGAL_ADDRESS` and `LEGAL_EMAIL` environment variables.

use axum::{extract::State, response::IntoResponse};
use maud::{Markup, html};

use crate::{AppState, views};

/// Last time these texts changed.
const UPDATED: &str = "11 November 2026";

/// Who runs the panel, as configured.
#[derive(Debug, Clone, Default)]
pub struct Operator {
    pub name: Option<String>,
    pub address: Option<String>,
    pub email: Option<String>,
}

impl Operator {
    pub fn from_env() -> Self {
        let read = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Self {
            name: read("LEGAL_NAME"),
            address: read("LEGAL_ADDRESS"),
            email: read("LEGAL_EMAIL"),
        }
    }

    fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("The operator of this site")
    }

    fn contact(&self) -> Markup {
        html! {
            @match &self.email {
                Some(email) => a href={ "mailto:" (email) } { (email) },
                None => "the contact address given in the legal notice",
            }
        }
    }
}

fn hosting() -> Markup {
    html! {
        p {
            "netcup GmbH" br;
            "Emmy-Noether-Straße 10" br;
            "76131 Karlsruhe, Germany" br;
            a href="https://www.netcup.com" rel="noopener noreferrer" { "www.netcup.com" }
        }
    }
}

pub async fn notice(State(state): State<AppState>) -> impl IntoResponse {
    let operator = &state.legal;
    let body = html! {
        h2 { "Publisher" }
        p {
            (operator.name())
            @if let Some(address) = &operator.address {
                br;
                @for (index, line) in address.split(',').enumerate() {
                    @if index > 0 { br; }
                    (line.trim())
                }
            }
        }
        p { "Contact: " (operator.contact()) }
        h2 { "Hosting" }
        p { "The site and the bots run on servers rented from:" }
        (hosting())
        h2 { "Discord" }
        p {
            "This site and its bots are independent. They are not made, endorsed or "
            "supported by Discord Inc. Discord is a trademark of Discord Inc."
        }
        h2 { "Intellectual property" }
        p {
            "The bots' images use the Inter typeface (SIL Open Font License). Avatars, "
            "server names and icons belong to their owners and are shown from Discord."
        }
    };
    views::legal_page("Legal notice", UPDATED, body)
}

pub async fn privacy(State(state): State<AppState>) -> impl IntoResponse {
    let operator = &state.legal;
    let body = html! {
        p {
            "This policy explains what personal data the panel at dc.enderr.win and the "
            "Discord bots it manages process, why, and what your rights are."
        }
        h2 { "Controller" }
        p { (operator.name()) ", reachable at " (operator.contact()) ". See the legal notice." }

        h2 { "When you log in to the panel" }
        ul {
            li { strong { "Discord account: " } "your Discord id, username, display name and avatar, read through Discord's login (the " code { "identify" } " scope only). Your e-mail address, servers and messages are never requested." }
            li { strong { "Session: " } "a cookie that keeps you logged in. Only a hash of it is stored on the server. It expires after 30 days or when you log out." }
            li { strong { "Purpose: " } "showing you the bots assigned to you and letting you manage them (performance of the service you asked for)." }
        }

        h2 { "When a bot is on your Discord server" }
        ul {
            li { strong { "Server settings " } "chosen on the panel or with the bot's commands (channels, roles, texts)." }
            li { strong { "Levels: " } "the id, display name and XP of members who chat, to compute levels and the leaderboard." }
            li { strong { "Logs: " } "the bot keeps the most recent messages in memory only (never on disk) so it can report deleted or edited messages in the log channel chosen by the server's staff." }
            li { strong { "Tickets: " } "who opened a ticket and when. When a ticket is closed, its transcript is posted in the server's transcript channel, not kept by the bot." }
            li { strong { "Giveaways: " } "who entered and who won." }
            li { strong { "Verification: " } "the captcha and the member's id, in memory, for 10 minutes at most." }
            li { strong { "Purpose: " } "running the features the server's administrators turned on (legitimate interest of the server and of its members)." }
        }

        h2 { "How long data is kept" }
        p {
            "Sessions: 30 days at most. Server settings, levels, tickets and giveaways: as long "
            "as the bot is used on the server. They can be erased at any time from the "
            "dashboard's Danger zone or on request. The hosting provider may keep technical "
            "logs (such as IP addresses) for a short time to keep the servers secure."
        }

        h2 { "Who receives it" }
        p {
            "Nobody buys or receives your data for advertising. It is processed by the "
            "hosting provider below and exchanged with Discord, which runs the platform the "
            "bots work on (Discord Inc., United States, under its own privacy policy)."
        }
        (hosting())

        h2 { "Cookies and storage" }
        p {
            "The panel only uses cookies needed for it to work: the session cookie, and a "
            "short-lived cookie during login. Your browser also keeps a copy of the "
            "\"Recovering connection\" page so it can show it while the panel restarts. "
            "There is no tracking and no analytics."
        }

        h2 { "Your rights" }
        p {
            "Under the GDPR you can ask to access, correct or erase your data, to restrict or "
            "object to its processing, and to receive it in a portable format. Write to "
            (operator.contact()) ". You can also complain to a data protection authority "
            "(in France, the CNIL: "
            a href="https://www.cnil.fr" rel="noopener noreferrer" { "www.cnil.fr" }
            ")."
        }
    };
    views::legal_page("Privacy policy", UPDATED, body)
}

pub async fn terms(State(state): State<AppState>) -> impl IntoResponse {
    let operator = &state.legal;
    let body = html! {
        h2 { "The service" }
        p {
            "dc.enderr.win lets people manage the Discord bots that " (operator.name())
            " assigned to them. Access is given by the operator; logging in with Discord "
            "alone does not give access to any bot."
        }
        h2 { "Your use" }
        ul {
            li { "Use the bots and the panel in line with the law and with Discord's Terms of Service and Community Guidelines." }
            li { "You are responsible for the settings you choose and for what the bot posts on your server because of them (announcements, mentions, sanctions)." }
            li { "Do not try to access bots or servers that are not yours, or to disrupt the service." }
        }
        h2 { "Availability" }
        p {
            "The service is provided as is. It may be interrupted for maintenance or "
            "updates, and features may change. The operator may suspend access in case of "
            "abuse."
        }
        h2 { "Liability" }
        p {
            "The operator is not liable for Discord outages, for actions taken by server "
            "administrators with the bots, or for indirect damage, within the limits "
            "allowed by law."
        }
        h2 { "Contact" }
        p { "Questions about these terms: " (operator.contact()) "." }
    };
    views::legal_page("Terms of use", UPDATED, body)
}
