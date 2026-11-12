//! State of a Minecraft Java server (ported from the Misty bot) with the
//! "Server List Ping" protocol: players, version, message of the day, icon
//! and ping. The address can be `host` or `host:port`; the
//! `_minecraft._tcp` SRV record is followed like the game does (resolved
//! with DNS over HTTPS).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use dc_core::{cards, guild::Minecraft};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::ui::{self, Msg};

const TIMEOUT: Duration = Duration::from_secs(3);
/// Answers kept, so clicks do not hammer the server.
const CACHE: Duration = Duration::from_secs(30);
pub const REFRESH: &str = "smp:refresh";

#[derive(Clone, Debug, Default)]
pub struct ServerStatus {
    pub online: bool,
    pub players: u64,
    pub max_players: u64,
    /// A few names (servers only give a sample).
    pub sample: Vec<String>,
    pub version: String,
    pub motd: Vec<String>,
    pub favicon: Option<Vec<u8>>,
    pub latency_ms: u64,
}

#[derive(Default)]
pub struct Pinger {
    cache: Mutex<HashMap<String, (Instant, ServerStatus)>>,
}

impl Pinger {
    /// State of the server (offline when unreachable), cached 30 seconds.
    pub async fn status(&self, http: &reqwest::Client, address: &str) -> ServerStatus {
        let key = address.trim().to_lowercase();
        if let Some((at, status)) = self.cache.lock().unwrap().get(&key)
            && at.elapsed() < CACHE
        {
            return status.clone();
        }
        let status = match tokio::time::timeout(TIMEOUT * 2, query(http, &key)).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                tracing::debug!("Minecraft server {key}: {error:#}");
                ServerStatus::default()
            }
            Err(_) => ServerStatus::default(),
        };
        let mut cache = self.cache.lock().unwrap();
        if cache.len() > 200 {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), status.clone()));
        status
    }
}

/// `host:port` (25565 by default).
fn split(address: &str) -> (String, Option<u16>) {
    match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host.to_owned(), Some(port)),
            Err(_) => (address.to_owned(), None),
        },
        _ => (address.to_owned(), None),
    }
}

/// SRV record `_minecraft._tcp.<host>` (DNS over HTTPS, Cloudflare).
async fn srv(http: &reqwest::Client, host: &str) -> Option<(String, u16)> {
    let url = format!("https://cloudflare-dns.com/dns-query?name=_minecraft._tcp.{host}&type=SRV");
    let answer: Value = http
        .get(url)
        .header("Accept", "application/dns-json")
        .timeout(TIMEOUT)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    // "priority weight port target."
    let data = answer
        .get("Answer")?
        .as_array()?
        .first()?
        .get("data")?
        .as_str()?;
    let parts: Vec<&str> = data.split_whitespace().collect();
    let port = parts.get(2)?.parse().ok()?;
    let target = parts.get(3)?.trim_end_matches('.').to_owned();
    (!target.is_empty()).then_some((target, port))
}

fn varint(mut value: i32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value = ((value as u32) >> 7) as i32;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

async fn read_varint(stream: &mut TcpStream) -> Result<i32> {
    let mut value = 0i32;
    for i in 0..5 {
        let byte = stream.read_u8().await?;
        value |= i32::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("varint too long")
}

fn packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut inner = Vec::new();
    varint(id, &mut inner);
    inner.extend_from_slice(body);
    let mut out = Vec::new();
    varint(inner.len() as i32, &mut out);
    out.extend(inner);
    out
}

async fn query(http: &reqwest::Client, address: &str) -> Result<ServerStatus> {
    let (host, port) = split(address);
    if host.is_empty() {
        bail!("no address");
    }
    let (connect_host, connect_port) = match port {
        Some(port) => (host.clone(), port),
        None => srv(http, &host).await.unwrap_or((host.clone(), 25565)),
    };
    let mut stream = tokio::time::timeout(
        TIMEOUT,
        TcpStream::connect((connect_host.as_str(), connect_port)),
    )
    .await
    .context("timed out")??;

    // Handshake (status state), then the status request.
    let mut handshake = Vec::new();
    varint(-1, &mut handshake);
    varint(host.len() as i32, &mut handshake);
    handshake.extend_from_slice(host.as_bytes());
    handshake.extend_from_slice(&connect_port.to_be_bytes());
    varint(1, &mut handshake);
    stream.write_all(&packet(0, &handshake)).await?;
    stream.write_all(&packet(0, &[])).await?;

    let read = async {
        let _length = read_varint(&mut stream).await?;
        if read_varint(&mut stream).await? != 0 {
            bail!("unexpected answer");
        }
        let n = read_varint(&mut stream).await? as usize;
        if n > 1 << 21 {
            bail!("answer too large");
        }
        let mut buf = vec![0; n];
        stream.read_exact(&mut buf).await?;
        let start = Instant::now();
        stream.write_all(&packet(1, &42i64.to_be_bytes())).await?;
        let _ = read_varint(&mut stream).await?;
        let latency = start.elapsed().as_millis() as u64;
        Ok::<_, anyhow::Error>((buf, latency))
    };
    let (buf, latency) = tokio::time::timeout(TIMEOUT, read)
        .await
        .context("timed out")??;
    let json: Value = serde_json::from_slice(&buf)?;
    Ok(parse(&json, latency))
}

/// Text of a Minecraft chat component (string, `text` + `extra`, or list).
fn chat_text(value: &Value, out: &mut String) {
    match value {
        Value::String(s) => out.push_str(s),
        Value::Array(items) => items.iter().for_each(|x| chat_text(x, out)),
        Value::Object(map) => {
            if let Some(text) = map.get("text") {
                chat_text(text, out);
            }
            if let Some(extra) = map.get("extra") {
                chat_text(extra, out);
            }
        }
        _ => {}
    }
}

/// Removes the `§x` color codes.
fn strip_codes(s: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for c in s.chars() {
        if skip {
            skip = false;
        } else if c == '§' {
            skip = true;
        } else {
            out.push(c);
        }
    }
    out
}

fn parse(json: &Value, latency_ms: u64) -> ServerStatus {
    use base64::Engine;
    let mut motd = String::new();
    if let Some(description) = json.get("description") {
        chat_text(description, &mut motd);
    }
    let motd = strip_codes(&motd)
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    let players = json.get("players");
    let favicon = json
        .get("favicon")
        .and_then(Value::as_str)
        .and_then(|f| f.split_once("base64,"))
        .and_then(|(_, b)| {
            base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .ok()
        });
    let count = |key: &str| {
        players
            .and_then(|p| p.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    ServerStatus {
        online: true,
        players: count("online"),
        max_players: count("max"),
        sample: players
            .and_then(|p| p.get("sample"))
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|x| x.get("name")?.as_str())
                    .map(strip_codes)
                    .filter(|n| !n.trim().is_empty())
                    .take(10)
                    .collect()
            })
            .unwrap_or_default(),
        version: json
            .get("version")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .map(strip_codes)
            .unwrap_or_default(),
        motd,
        favicon,
        latency_ms,
    }
}

/// The server's card: image (state, players, version), address, players
/// online, refresh button.
pub fn card(settings: &Minecraft, status: &ServerStatus) -> Msg {
    let mut numbers = Vec::new();
    if status.online {
        numbers.push((
            "Players".to_owned(),
            format!(
                "{} / {}",
                ui::number(status.players),
                ui::number(status.max_players)
            ),
        ));
        if !status.version.is_empty() {
            numbers.push(("Version".to_owned(), status.version.clone()));
        }
        numbers.push(("Ping".to_owned(), format!("{} ms", status.latency_ms)));
    }
    let motd = if status.online {
        status.motd.clone()
    } else {
        vec!["The server is not answering right now.".to_owned()]
    };
    let svg = cards::server(&cards::ServerCard {
        label: "Minecraft server",
        name: &settings.name,
        address: &settings.address,
        online: status.online,
        motd: &motd,
        icon: status.favicon.as_deref(),
        numbers: &numbers,
    });
    let mut children = Vec::new();
    if !settings.description.trim().is_empty() {
        children.push(ui::text(ui::truncate(settings.description.trim(), 1500)));
    }
    let mut lines = vec![format!(
        "**Address** `{}`",
        settings.address.replace('`', "")
    )];
    if status.online && settings.show_players && !status.sample.is_empty() {
        let names: Vec<String> = status.sample.iter().map(|n| ui::escape(n)).collect();
        lines.push(format!("**Online** {}", names.join(", ")));
    }
    lines.push(format!("-# Checked <t:{}:R>", dc_core::now()));
    children.push(ui::separator());
    children.push(ui::text(lines.join("\n")));
    children.push(ui::row(vec![ui::button(
        ui::STYLE_SECONDARY,
        "Refresh",
        REFRESH,
    )]));
    let fallback = vec![ui::text(format!("## {}", ui::escape(&settings.name)))];
    let color = if status.online { ui::GREEN } else { ui::RED };
    Msg::with_image(color, "server.png", svg, children, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_status() {
        let status = parse(
            &json!({
                "version": {"name": "Paper 1.21.4", "protocol": 769},
                "players": {"max": 50, "online": 3, "sample": [{"name": "Ender", "id": "x"}]},
                "description": {"text": "§bMisty §fSMP", "extra": [{"text": "\nSurvival"}]},
                "favicon": "data:image/png;base64,iVBORw0KGgo="
            }),
            12,
        );
        assert!(status.online);
        assert_eq!((status.players, status.max_players), (3, 50));
        assert_eq!(status.sample, vec!["Ender"]);
        assert_eq!(status.version, "Paper 1.21.4");
        assert_eq!(status.motd, vec!["Misty SMP", "Survival"]);
        assert!(status.favicon.unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn addresses_varints_and_card() {
        assert_eq!(split("play.misty.fr"), ("play.misty.fr".into(), None));
        assert_eq!(split("1.2.3.4:25570"), ("1.2.3.4".into(), Some(25570)));
        let mut v = Vec::new();
        varint(-1, &mut v);
        assert_eq!(v, [0xff, 0xff, 0xff, 0xff, 0x0f]);
        let mut v = Vec::new();
        varint(300, &mut v);
        assert_eq!(v, [0xac, 0x02]);

        let settings = Minecraft {
            address: "play.misty.fr".into(),
            ..Minecraft::default()
        };
        let online = ServerStatus {
            online: true,
            players: 3,
            max_players: 50,
            sample: vec!["Ender_*".into()],
            ..ServerStatus::default()
        };
        let text = card(&settings, &online).payload().to_string();
        assert!(text.contains("play.misty.fr") && text.contains("Ender\\\\_\\\\*"));
        let offline = card(&settings, &ServerStatus::default())
            .payload()
            .to_string();
        assert!(offline.contains(REFRESH) && !offline.contains("**Online**"));
    }
}

/// Real network: `PREVIEW_DIR=… MC_ADDR=mc.hypixel.net cargo test -p dc-bot live_ -- --ignored`.
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_minecraft_and_github() {
        let dir = std::env::var("PREVIEW_DIR").unwrap();
        let background = std::fs::read(std::env::var("BACKGROUND").unwrap()).unwrap();
        dc_core::render::set_background(dc_core::render::Background::from_bytes(&background, 30));
        let http = reqwest::Client::new();
        let address = std::env::var("MC_ADDR").unwrap();
        let status = Pinger::default().status(&http, &address).await;
        println!(
            "{address}: online={} {}/{} {:?} {} ms",
            status.online, status.players, status.max_players, status.version, status.latency_ms
        );
        assert!(status.online);
        let settings = Minecraft {
            address,
            name: "Misty SMP".into(),
            ..Minecraft::default()
        };
        let msg = card(&settings, &status).finish().await;
        std::fs::write(format!("{dir}/server.png"), &msg.files[0].1).unwrap();

        let gh = crate::github::GitHub::new(http, None);
        let releases = gh.releases("Misty-MC/Misty").await.unwrap();
        let latest = crate::github::latest(&releases).unwrap();
        println!(
            "releases: {} latest {} downloads {}",
            releases.len(),
            latest.tag_name,
            latest.downloads()
        );
        let repo = gh.repo("Misty-MC/Misty").await.unwrap();
        let commits = gh
            .commits("Misty-MC/Misty", &repo.default_branch)
            .await
            .unwrap();
        println!(
            "stars {} branch {} commits {}",
            repo.stargazers_count,
            repo.default_branch,
            commits.len()
        );
        let settings = dc_core::guild::Releases {
            download_url: "https://misty-launcher.online/download/setup".into(),
            ..Default::default()
        };
        let msg = crate::ghfeed::release_card(&settings, "Misty-MC/Misty", latest, false)
            .finish()
            .await;
        std::fs::write(format!("{dir}/release.png"), &msg.files[0].1).unwrap();
        // Cached: a second call costs no request.
        assert_eq!(
            gh.releases("Misty-MC/Misty").await.unwrap().len(),
            releases.len()
        );
        let rows = vec![
            dc_core::cards::SiteRow {
                name: "Site".into(),
                detail: "misty-launcher.online".into(),
                status: Some(200),
                latency_ms: 120,
                history: vec![dc_core::cards::Beat::Up; 30],
            },
            dc_core::cards::SiteRow {
                name: "API".into(),
                detail: "misty-launcher.online/api/latest".into(),
                status: Some(200),
                latency_ms: 1800,
                history: vec![dc_core::cards::Beat::Slow; 3],
            },
        ];
        let numbers = vec![
            ("Version".to_owned(), latest.tag_name.clone()),
            (
                "Downloads".to_owned(),
                crate::ui::number(latest.downloads()),
            ),
            ("Stars".to_owned(), crate::ui::number(repo.stargazers_count)),
        ];
        let svg = dc_core::cards::status("Misty Bot", "Status", &rows, &numbers);
        std::fs::write(
            format!("{dir}/status.png"),
            dc_core::render::png(&svg).unwrap(),
        )
        .unwrap();
    }
}
