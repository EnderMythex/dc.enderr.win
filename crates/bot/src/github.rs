//! Minimal GitHub client (ported from the Misty bot): releases, repository
//! information and the lists the activity feed compares between checks.
//!
//! Every request is conditional (ETag): a "not modified" answer does not
//! count in GitHub's quota, so repositories can be checked every minute.
//! `GITHUB_TOKEN` (a token without any permission) raises the quota from 60
//! to 5,000 requests per hour; the repositories must be public either way.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone, Debug)]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub body: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub download_count: u64,
}

impl Release {
    pub fn downloads(&self) -> u64 {
        self.assets.iter().map(|a| a.download_count).sum()
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Actor {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct RepoInfo {
    #[serde(default)]
    pub stargazers_count: u64,
    #[serde(default)]
    pub default_branch: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitDetail,
    pub author: Option<Actor>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CommitDetail {
    pub message: String,
    pub author: Option<GitSignature>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct GitSignature {
    pub name: Option<String>,
}

/// A pull request or an issue (same useful fields).
#[derive(Deserialize, Clone, Debug)]
pub struct Ticket {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    pub user: Option<Actor>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<Actor>,
    #[serde(default)]
    pub closed_by: Option<Actor>,
    /// Present on the issues that are pull requests.
    #[serde(default)]
    pub pull_request: Option<Value>,
}

impl Ticket {
    /// `open`, `closed` or `merged`.
    pub fn status(&self) -> &str {
        if self.merged_at.is_some() {
            "merged"
        } else {
            &self.state
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Fork {
    pub id: u64,
    pub full_name: String,
    pub owner: Actor,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Named {
    pub name: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Stargazer {
    pub user: Actor,
}

/// A kept answer: ETag, body, when it was last confirmed.
type Cached = (String, Vec<u8>, Instant);

/// The quota is spent: no request before this Unix time.
#[derive(Debug)]
pub struct RateLimited(pub u64);

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub quota spent until <{}>", self.0)
    }
}

impl std::error::Error for RateLimited {}

/// The error comes from GitHub's quota (logged once, not at every check).
pub fn is_rate_limited(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<RateLimited>())
}

pub struct GitHub {
    http: reqwest::Client,
    token: Option<String>,
    etags: Mutex<HashMap<String, Cached>>,
    /// False once GitHub refused the token: it is not sent anymore.
    token_ok: AtomicBool,
    blocked_until: Mutex<Option<u64>>,
}

fn unix_now() -> u64 {
    u64::try_from(dc_core::now()).unwrap_or(0)
}

impl GitHub {
    pub fn new(http: reqwest::Client, token: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            token_ok: AtomicBool::new(token.is_some()),
            http,
            token,
            etags: Mutex::default(),
            blocked_until: Mutex::default(),
        })
    }

    /// A valid token is used (5,000 requests per hour instead of 60).
    pub fn authenticated(&self) -> bool {
        self.token_ok.load(Ordering::Relaxed)
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        self.get_as(path, "application/vnd.github+json").await
    }

    async fn get_as<T: for<'de> Deserialize<'de>>(&self, path: &str, accept: &str) -> Result<T> {
        let key = format!("{accept} {path}");
        if let Some(until) = *self.blocked_until.lock().unwrap()
            && unix_now() < until
        {
            return Err(RateLimited(until).into());
        }
        // An answer under 30 seconds old: no request at all.
        let fresh = self
            .etags
            .lock()
            .unwrap()
            .get(&key)
            .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(30))
            .map(|(_, body, _)| body.clone());
        if let Some(body) = fresh
            && let Ok(value) = serde_json::from_slice(&body)
        {
            return Ok(value);
        }
        let etag = self
            .etags
            .lock()
            .unwrap()
            .get(&key)
            .map(|(etag, _, _)| etag.clone());
        let with_token = self.authenticated();
        let mut response = self
            .request(path, accept, etag.as_deref(), with_token)
            .await?;
        // Token refused (expired, or blocked by the organization): said once,
        // then only anonymous requests.
        if with_token && matches!(response.status().as_u16(), 401 | 403) && !is_quota(&response) {
            self.token_ok.store(false, Ordering::Relaxed);
            tracing::error!(
                "GitHub refused GITHUB_TOKEN; continuing without it (60 requests per hour)"
            );
            response = self.request(path, accept, etag.as_deref(), false).await?;
        }
        if is_quota(&response) {
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok()?.parse().ok())
                .unwrap_or_else(|| unix_now() + 600);
            let first = self
                .blocked_until
                .lock()
                .unwrap()
                .replace(reset)
                .is_none_or(|old| old < unix_now());
            if first {
                tracing::warn!(
                    minutes = reset.saturating_sub(unix_now()).div_ceil(60),
                    token = self.authenticated(),
                    "GitHub quota spent, waiting for its reset"
                );
            }
            return Err(RateLimited(reset).into());
        }
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            let mut cache = self.etags.lock().unwrap();
            if let Some((_, body, at)) = cache.get_mut(&key) {
                *at = Instant::now();
                return Ok(serde_json::from_slice(body)?);
            }
        }
        if !status.is_success() {
            anyhow::bail!("GitHub answered {status} for {path}");
        }
        let new_etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.bytes().await?.to_vec();
        let parsed = serde_json::from_slice(&body)?;
        if let Some(etag) = new_etag {
            let mut cache = self.etags.lock().unwrap();
            if cache.len() > 500 {
                cache.clear();
            }
            cache.insert(key, (etag, body, Instant::now()));
        }
        Ok(parsed)
    }

    async fn request(
        &self,
        path: &str,
        accept: &str,
        etag: Option<&str>,
        with_token: bool,
    ) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .get(format!("https://api.github.com{path}"))
            .header("Accept", accept)
            .header("User-Agent", "dc-bot")
            .timeout(Duration::from_secs(20));
        if with_token && let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        Ok(request.send().await?)
    }

    /// Published releases (no drafts), newest first.
    pub async fn releases(&self, repo: &str) -> Result<Vec<Release>> {
        let mut list: Vec<Release> = self
            .get(&format!("/repos/{repo}/releases?per_page=100"))
            .await?;
        list.retain(|r| !r.draft);
        Ok(list)
    }

    pub async fn repo(&self, repo: &str) -> Result<RepoInfo> {
        self.get(&format!("/repos/{repo}")).await
    }

    pub async fn commits(&self, repo: &str, branch: &str) -> Result<Vec<Commit>> {
        self.get(&format!("/repos/{repo}/commits?sha={branch}&per_page=30"))
            .await
    }

    pub async fn pulls(&self, repo: &str) -> Result<Vec<Ticket>> {
        self.get(&format!(
            "/repos/{repo}/pulls?state=all&sort=updated&direction=desc&per_page=20"
        ))
        .await
    }

    pub async fn pull(&self, repo: &str, number: u64) -> Result<Ticket> {
        self.get(&format!("/repos/{repo}/pulls/{number}")).await
    }

    /// Recently changed issues (without the pull requests).
    pub async fn issues(&self, repo: &str) -> Result<Vec<Ticket>> {
        let mut list: Vec<Ticket> = self
            .get(&format!(
                "/repos/{repo}/issues?state=all&sort=updated&direction=desc&per_page=20"
            ))
            .await?;
        list.retain(|t| t.pull_request.is_none());
        Ok(list)
    }

    pub async fn issue(&self, repo: &str, number: u64) -> Result<Ticket> {
        self.get(&format!("/repos/{repo}/issues/{number}")).await
    }

    pub async fn forks(&self, repo: &str) -> Result<Vec<Fork>> {
        self.get(&format!("/repos/{repo}/forks?sort=newest&per_page=10"))
            .await
    }

    pub async fn tags(&self, repo: &str) -> Result<Vec<Named>> {
        self.get(&format!("/repos/{repo}/tags?per_page=30")).await
    }

    pub async fn branches(&self, repo: &str) -> Result<Vec<Named>> {
        self.get(&format!("/repos/{repo}/branches?per_page=100"))
            .await
    }

    /// The n-th stargazer (1 = the first), to know who just starred.
    pub async fn stargazer(&self, repo: &str, n: u64) -> Result<Option<Stargazer>> {
        let list: Vec<Stargazer> = self
            .get_as(
                &format!("/repos/{repo}/stargazers?per_page=1&page={n}"),
                "application/vnd.github.star+json",
            )
            .await?;
        Ok(list.into_iter().next())
    }
}

/// A "quota spent" answer (403 or 429 with `x-ratelimit-remaining: 0`).
fn is_quota(response: &reqwest::Response) -> bool {
    matches!(response.status().as_u16(), 403 | 429)
        && response
            .headers()
            .get("x-ratelimit-remaining")
            .is_some_and(|v| v.as_bytes() == b"0")
}

/// The version shown: the newest stable one, else the newest.
pub fn latest(list: &[Release]) -> Option<&Release> {
    list.iter().find(|r| !r.prerelease).or_else(|| list.first())
}

/// ISO 8601 date -> Unix time (for Discord's `<t:…:R>`).
pub fn unix(iso: Option<&str>) -> Option<i64> {
    crate::members::parse_timestamp(iso?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_latest() {
        assert_eq!(unix(Some("2026-11-07T21:40:18Z")), Some(1_794_087_618));
        assert_eq!(unix(Some("nope")), None);
        let release = |id, prerelease| Release {
            id,
            tag_name: format!("v{id}"),
            body: None,
            html_url: String::new(),
            published_at: None,
            prerelease,
            draft: false,
            assets: Vec::new(),
        };
        let list = vec![release(3, true), release(2, false)];
        assert_eq!(latest(&list).unwrap().id, 2);
        assert_eq!(latest(&list[..1]).unwrap().id, 3);
    }
}
