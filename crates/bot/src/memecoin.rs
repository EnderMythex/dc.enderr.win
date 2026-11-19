//! Token contract addresses ("CA" of a memecoin) posted by members who
//! joined recently: the message is deleted, the author timed out briefly and
//! told why in a private message. Wallet addresses are left alone. A wallet
//! and a token look the same, so the bot asks:
//! - Solana itself: a token is a "mint" account, a wallet is not;
//! - DexScreener for every chain (Ethereum, Base, BSC, Arbitrum, Tron, TON,
//!   Sui…, and Solana tokens not caught above): a token has trading pairs, a
//!   wallet has none.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    antiscam,
    app::{App, str_of},
    linkguard, logs, members,
    ui::Msg,
};

const DEFAULT_RPC: &str = "https://api.mainnet-beta.solana.com";
/// Pairs of up to 30 tokens, on every chain.
const DEXSCREENER: &str = "https://api.dexscreener.com/latest/dex/tokens/";
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

/// EVM (Ethereum, Base, BSC, Arbitrum, Polygon, Avalanche…): `0x` + 40 hex.
static EVM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b0x[0-9a-fA-F]{40}\b").unwrap());
/// Sui coin type: `0x…::module::NAME`.
static SUI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b0x[0-9a-fA-F]{64}::[A-Za-z_][A-Za-z0-9_]*::[A-Za-z_][A-Za-z0-9_]*").unwrap()
});
/// TON: `EQ…` or `UQ…`, 48 characters.
static TON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_-])([EU]Q[A-Za-z0-9_-]{46})(?:$|[^A-Za-z0-9_-])").unwrap()
});

/// Solana and Tron addresses: 32 to 44 base58 characters.
fn is_solana_shaped(address: &str) -> bool {
    (32..=44).contains(&address.len()) && address.chars().all(is_base58)
}

/// The blockchain addresses written in a message (links included: a
/// pump.fun, Dexscreener or Etherscan link carries the address too).
pub fn candidates(content: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |address: &str| {
        if found.len() < MAX_CANDIDATES && !found.iter().any(|f| f.eq_ignore_ascii_case(address)) {
            found.push(address.to_owned());
        }
    };
    for m in SUI.find_iter(content) {
        add(m.as_str());
    }
    for m in EVM.find_iter(content) {
        add(m.as_str());
    }
    for caps in TON.captures_iter(content) {
        add(&caps[1]);
    }
    for word in content
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| is_solana_shaped(word))
    {
        // A real address mixes letters and digits; a long plain word does not.
        if word.chars().filter(char::is_ascii_digit).count() >= 2 {
            add(word);
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

/// Without any answer: pump.fun-style addresses ("...pump") are tokens,
/// anything else is given the benefit of the doubt.
fn guess(address: &str) -> bool {
    address.ends_with("pump") || address.ends_with("bonk")
}

/// Which of these Solana addresses are token mints; `None` if Solana did
/// not answer.
async fn solana_mints(app: &App, addresses: &[String]) -> Option<Vec<bool>> {
    let rpc = std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_owned());
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "getMultipleAccounts",
        "params": [addresses, { "encoding": "jsonParsed" }],
    });
    let response = app
        .web
        .post(&rpc)
        .json(&body)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .ok()
        .filter(|response| response.status().is_success())?;
    let answer: Value = response.json().await.ok()?;
    let accounts = answer["result"]["value"].as_array()?;
    (accounts.len() == addresses.len()).then(|| accounts.iter().map(is_mint).collect())
}

/// The addresses (lowercase) that are tokens with trading pairs on
/// DexScreener, any chain; `None` if DexScreener did not answer.
fn listed_tokens(answer: &Value) -> Vec<String> {
    let mut listed = Vec::new();
    for pair in answer["pairs"].as_array().into_iter().flatten() {
        for side in ["baseToken", "quoteToken"] {
            if let Some(address) = pair[side]["address"].as_str() {
                listed.push(address.to_lowercase());
            }
        }
    }
    listed
}

async fn dexscreener(app: &App, addresses: &[String]) -> Option<Vec<String>> {
    let url = format!("{DEXSCREENER}{}", addresses.join(","));
    let response = app
        .web
        .get(&url)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .ok()
        .filter(|response| response.status().is_success())?;
    let answer: Value = response.json().await.ok()?;
    Some(listed_tokens(&answer))
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
        let mut verdicts: HashMap<String, bool> = HashMap::new();
        // 1. Solana: a mint is a token, even before it trades anywhere.
        let solana: Vec<String> = unknown
            .iter()
            .filter(|a| is_solana_shaped(a))
            .cloned()
            .collect();
        if !solana.is_empty() {
            match solana_mints(app, &solana).await {
                Some(mints) => {
                    for (address, mint) in solana.iter().zip(mints) {
                        if mint {
                            verdicts.insert(address.clone(), true);
                        }
                    }
                }
                None => tracing::warn!("Solana did not answer"),
            }
        }
        // 2. Every chain: a token has trading pairs, a wallet has none.
        let rest: Vec<String> = unknown
            .iter()
            .filter(|a| !verdicts.contains_key(*a))
            .cloned()
            .collect();
        if !rest.is_empty() {
            match dexscreener(app, &rest).await {
                Some(listed) => {
                    for address in rest {
                        let token = listed.contains(&address.to_lowercase());
                        verdicts.insert(address, token);
                    }
                }
                None => tracing::warn!("DexScreener did not answer"),
            }
        }
        // Undecided addresses are not remembered: asked again next time.
        let mut known = KNOWN.lock().unwrap();
        if known.len() > MAX_REMEMBERED {
            known.clear();
        }
        known.extend(verdicts);
    }
    let known = KNOWN.lock().unwrap();
    addresses
        .iter()
        .filter(|a| known.get(*a).copied().unwrap_or_else(|| guess(a)))
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
        // Other chains.
        let pepe = "0x6982508145454Ce325dDbE47a25d4ec3d2311933";
        assert_eq!(candidates(&format!("ca: {pepe}")), vec![pepe]);
        assert_eq!(
            candidates(&format!("https://etherscan.io/token/{pepe}?a=1")),
            vec![pepe]
        );
        let sui = format!("0x{}::coin::DOGE", "a1".repeat(32));
        assert_eq!(candidates(&format!("sui {sui} !")), vec![sui.clone()]);
        let ton = format!(
            "EQ{}",
            "Ab9_-x".repeat(8).chars().take(46).collect::<String>()
        );
        assert_eq!(candidates(&format!("ton {ton}")), vec![ton]);
        let tron = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
        assert_eq!(candidates(tron), vec![tron]);
        // A transaction hash (64 hex) is not an address.
        assert!(candidates(&format!("tx 0x{}", "ab".repeat(32))).is_empty());
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
    fn dexscreener_pairs() {
        let answer = json!({ "pairs": [
            { "chainId": "ethereum", "baseToken": { "address": "0x6982508145454Ce325dDbE47a25d4ec3d2311933" },
              "quoteToken": { "address": "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2" } },
        ] });
        let listed = listed_tokens(&answer);
        assert!(listed.contains(&"0x6982508145454ce325ddbe47a25d4ec3d2311933".to_owned()));
        assert!(listed_tokens(&json!({ "pairs": null })).is_empty());
    }

    #[test]
    fn durations() {
        assert_eq!(duration(60, false), "1 min");
        assert_eq!(duration(7200, true), "2 h");
        assert_eq!(duration(172_800, true), "2 j");
    }
}
