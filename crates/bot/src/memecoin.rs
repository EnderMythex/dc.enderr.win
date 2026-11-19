//! Token contract addresses ("CA" of a memecoin) posted by members who
//! joined recently: the message is deleted, the author timed out briefly and
//! told why in a private message. Wallet addresses are left alone: Solana
//! tells the two apart (a token is a "mint" account, a wallet is not).

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    antiscam,
    app::{App, str_of},
    linkguard, logs, members,
    ui::Msg,
};

const DEFAULT_RPC: &str = "https://api.mainnet-beta.solana.com";
/// Addresses looked at per message.
const MAX_CANDIDATES: usize = 5;
/// Addresses whose kind is remembered.
const MAX_REMEMBERED: usize = 5_000;

/// Characters of a Solana address (base58: no 0, O, I or l).
fn is_base58(c: char) -> bool {
    c.is_ascii_alphanumeric() && !matches!(c, '0' | 'O' | 'I' | 'l')
}

/// Is each address a token (`true`) or something else, a wallet (`false`)?
static KNOWN: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);

/// The Solana addresses written in a message (links included: a pump.fun or
/// Dexscreener link carries the address too).
pub fn candidates(content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in content
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.chars().all(is_base58))
    {
        // A real address mixes letters and digits; a long plain word does not.
        let digits = word.chars().filter(char::is_ascii_digit).count();
        if (32..=44).contains(&word.len()) && digits >= 2 && !found.iter().any(|f| f == word) {
            found.push(word.to_owned());
            if found.len() >= MAX_CANDIDATES {
                break;
            }
        }
    }
    found
}

/// What an account of `getMultipleAccounts` (jsonParsed) is.
fn is_mint(account: &Value) -> bool {
    let data = &account["data"];
    let program = data["program"].as_str().unwrap_or_default();
    program.starts_with("spl-token") && data["parsed"]["type"] == "mint"
}

/// Without an answer from Solana: pump.fun-style addresses ("...pump") are
/// tokens, anything else is given the benefit of the doubt.
fn guess(address: &str) -> bool {
    address.ends_with("pump") || address.ends_with("bonk")
}

/// The tokens among these addresses.
async fn tokens(app: &App, addresses: &[String]) -> Vec<String> {
    let unknown: Vec<String> = {
        let known = KNOWN.lock().unwrap();
        addresses
            .iter()
            .filter(|a| !known.contains_key(*a))
            .cloned()
            .collect()
    };
    if !unknown.is_empty() {
        let rpc = std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_owned());
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "getMultipleAccounts",
            "params": [unknown, { "encoding": "jsonParsed" }],
        });
        let answer = app
            .web
            .post(&rpc)
            .json(&body)
            .timeout(Duration::from_secs(6))
            .send()
            .await;
        let parsed: Option<Value> = match answer {
            Ok(response) if response.status().is_success() => response.json().await.ok(),
            _ => None,
        };
        let accounts = parsed
            .as_ref()
            .and_then(|v| v["result"]["value"].as_array());
        let mut known = KNOWN.lock().unwrap();
        if known.len() > MAX_REMEMBERED {
            known.clear();
        }
        match accounts {
            Some(accounts) if accounts.len() == unknown.len() => {
                for (address, account) in unknown.iter().zip(accounts) {
                    known.insert(address.clone(), is_mint(account));
                }
            }
            _ => {
                tracing::warn!(
                    "Solana did not answer: contract addresses guessed from their suffix"
                );
                // Not remembered: Solana is asked again next time.
                drop(known);
                return addresses.iter().filter(|a| guess(a)).cloned().collect();
            }
        }
    }
    let known = KNOWN.lock().unwrap();
    addresses
        .iter()
        .filter(|a| known.get(*a).copied().unwrap_or(false))
        .cloned()
        .collect()
}

/// Days the author of a message has been on the server, if Discord says.
fn days_on_server(message: &Value) -> Option<i64> {
    let joined = members::parse_timestamp(str_of(&message["member"], "joined_at"))?;
    Some((dc_core::now() - joined).max(0) / 86_400)
}

/// Removes a message with a token address from a recent member. `true` when
/// the message was removed. Staff is checked by the caller.
pub async fn check(app: &App, guild: &str, message: &Value) -> bool {
    let config = app.config(guild);
    let rules = &config.anti_scam;
    if !rules.blocks_new_member_ca(&app.packs) {
        return false;
    }
    let min_days = i64::from(rules.ca_min_days);
    let Some(days) = days_on_server(message) else {
        return false;
    };
    if days >= min_days {
        return false;
    }
    let addresses = candidates(str_of(message, "content"));
    if addresses.is_empty() {
        return false;
    }
    let found = tokens(app, &addresses).await;
    let Some(address) = found.first() else {
        return false;
    };

    let user = str_of(&message["author"], "id");
    let channel = str_of(message, "channel_id");
    let reason = "Token address posted by a new member";
    let path = format!("/channels/{channel}/messages/{}", str_of(message, "id"));
    let _ = app.api.delete(&path, Some(reason)).await;
    let secs = i64::from(rules.ca_timeout_secs);
    let timed_out = secs > 0 && antiscam::timeout(app, guild, user, secs, reason).await;

    let left = (min_days - days).max(1);
    let server = app.guild(guild).name;
    let server = if server.is_empty() {
        "the server".to_owned()
    } else {
        server
    };
    let mute = if timed_out {
        format!(
            " Tu es mute {} / You are muted for {}.",
            duration(secs, true),
            duration(secs, false)
        )
    } else {
        String::new()
    };
    let body = format!(
        "**FR** · Ton message sur **{server}** a été supprimé : les membres arrivés depuis moins de {min_days} jours ne peuvent pas partager l'adresse (CA) d'un token. Tu pourras le faire dans {left} jour(s). Les adresses de wallet restent autorisées.\n\n\
         **EN** · Your message on **{server}** was removed: members who joined less than {min_days} days ago cannot share a token's contract address (CA). You can in {left} day(s). Wallet addresses are still allowed.{mute}"
    );
    if let Some(dm) = linkguard::dm_channel(app, user).await {
        let msg = Msg::warning("Contract address removed", &body);
        if let Err(error) = app.api.send_message(&dm, &msg).await {
            tracing::info!(%error, "the member does not accept DMs");
        }
    }
    let entry = format!(
        "**Member:** <@{user}> ({days} day(s) on the server)\n**Channel:** <#{channel}>\n**Address:** `{address}`\n**Result:** {}",
        if timed_out {
            format!("message deleted, timed out for {}", duration(secs, false))
        } else {
            "message deleted".to_owned()
        }
    );
    logs::moderation(app, guild, "Token address removed (new member)", &entry).await;
    true
}

/// "1 min" / "2 h", in French or English.
fn duration(secs: i64, french: bool) -> String {
    match secs {
        s if s < 3600 => format!("{} min", (s / 60).max(1)),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} {}", s / 86_400, if french { "j" } else { "d" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINT: &str = "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr";

    #[test]
    fn finds_addresses() {
        assert_eq!(candidates(&format!("buy {MINT} now")), vec![MINT]);
        let pump = "9BB6NFEcjBCtnNLFko2FqVQBq8HHM13kCyYcdQbgpump";
        assert_eq!(
            candidates(&format!("https://pump.fun/coin/{pump}")),
            vec![pump]
        );
        assert_eq!(
            candidates(&format!("{MINT}\n{pump} {MINT}")),
            vec![MINT.to_owned(), pump.to_owned()]
        );
        // Long words and short codes are not addresses.
        assert!(candidates("Supercalifragilisticexpialidociousandmore words").is_empty());
        assert!(candidates("my code AB12CD34").is_empty());
        // 0x addresses (EVM) are not Solana addresses.
        assert!(candidates("0x6982508145454Ce325dDbE47a25d4ec3d2311933").is_empty());
    }

    #[test]
    fn tells_mints_from_wallets() {
        let mint = json!({ "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            "data": { "program": "spl-token", "parsed": { "type": "mint" } } });
        let mint_2022 =
            json!({ "data": { "program": "spl-token-2022", "parsed": { "type": "mint" } } });
        let token_account =
            json!({ "data": { "program": "spl-token", "parsed": { "type": "account" } } });
        let wallet = json!({ "owner": "11111111111111111111111111111111", "data": ["", "base64"] });
        assert!(is_mint(&mint) && is_mint(&mint_2022));
        assert!(!is_mint(&token_account) && !is_mint(&wallet) && !is_mint(&Value::Null));
        assert!(guess("9BB6NFEcjBCtnNLFko2FqVQBq8HHM13kCyYcdQbgpump"));
        assert!(!guess(MINT));
    }

    #[test]
    fn durations() {
        assert_eq!(duration(60, false), "1 min");
        assert_eq!(duration(7200, true), "2 h");
        assert_eq!(duration(172_800, true), "2 j");
    }
}
