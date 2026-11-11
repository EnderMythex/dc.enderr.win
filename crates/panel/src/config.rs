use std::{env, path::PathBuf};

use anyhow::bail;

pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub public_url: String,
    pub bind_addr: String,
    pub database_url: String,
    /// Folder holding one sub-folder per bot (see `dc_core::registry`).
    pub bots_dir: PathBuf,
    /// Discord user ids that can see and manage every bot.
    pub admin_ids: Vec<String>,
    /// Bot definitions shipped with the image, copied into `bots_dir` at
    /// startup (see `dc_core::registry::seed`).
    pub seed_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let public_url = optional("PUBLIC_URL", "http://localhost:8080")
            .trim_end_matches('/')
            .to_owned();
        if !public_url.starts_with("http://") && !public_url.starts_with("https://") {
            bail!("PUBLIC_URL must start with http:// or https://");
        }
        let admin_ids = optional("ADMIN_IDS", "")
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            client_id: required("DISCORD_CLIENT_ID")?,
            client_secret: required("DISCORD_CLIENT_SECRET")?,
            public_url,
            bind_addr: optional("BIND_ADDR", "127.0.0.1:8080"),
            database_url: optional("DATABASE_URL", "sqlite://data/panel.db"),
            bots_dir: optional("BOTS_DIR", "bots").into(),
            admin_ids,
            seed_dir: env::var("BOTS_SEED_DIR")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from),
        })
    }

    pub fn redirect_uri(&self) -> String {
        format!("{}/auth/callback", self.public_url)
    }

    /// True when served over HTTPS: `Secure` cookies and HSTS.
    pub fn is_https(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    pub fn is_admin(&self, user_id: &str) -> bool {
        self.admin_ids.iter().any(|id| id == user_id)
    }
}

fn required(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => bail!("missing environment variable {name} (see .env.example)"),
    }
}

fn optional(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
        _ => default.to_owned(),
    }
}
