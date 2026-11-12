//! Per-IP rate limits of the panel (token buckets).
//!
//! - **Sensitive** requests (forms, login, verification, generated images):
//!   a burst of 20, then one every 3 seconds.
//! - **Everything else**: a burst of 120, then two per second.
//!
//! Behind a reverse proxy (Coolify's Traefik) the client's address is the
//! entry the proxy appended to `X-Forwarded-For`; `TRUSTED_PROXY_HOPS` says
//! how many proxies append to it (1 by default: Traefik only; 2 with
//! Cloudflare in front of Traefik). Earlier entries are ignored: a client
//! can write anything there.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, Method},
};

const MAX_TRACKED: usize = 50_000;
const FORGET_AFTER: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    General,
    Sensitive,
    /// The verification page and its captchas. Inside a Discord Activity every
    /// member comes through Discord's proxy, so they may all share one
    /// address: a wider allowance, the captchas being limited on their own.
    Verification,
}

impl Tier {
    /// `(burst, tokens per second)`.
    fn limits(self) -> (f64, f64) {
        match self {
            Self::General => (120.0, 2.0),
            Self::Sensitive => (20.0, 1.0 / 3.0),
            Self::Verification => (300.0, 5.0),
        }
    }

    pub fn of(method: &Method, path: &str) -> Option<Self> {
        let verification =
            path == "/activity" || path.starts_with("/activity/") || path.starts_with("/verify/");
        let static_file = path.starts_with("/assets/")
            || path == "/sw.js"
            || path == "/favicon.ico"
            || (verification && (path.ends_with("/app.js") || path.ends_with("/app.css")));
        if static_file {
            return None;
        }
        if verification {
            return Some(Self::Verification);
        }
        let sensitive =
            *method != Method::GET || path.starts_with("/auth/") || path.contains("/preview/");
        Some(if sensitive {
            Self::Sensitive
        } else {
            Self::General
        })
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Clone)]
pub struct Limiter {
    buckets: Arc<Mutex<HashMap<(IpAddr, Tier), Bucket>>>,
    hops: usize,
}

impl Limiter {
    pub fn new(hops: usize) -> Self {
        Self {
            buckets: Arc::default(),
            hops,
        }
    }

    /// Takes one token; `Err(seconds to wait)` when the bucket is empty.
    pub fn check(&self, ip: IpAddr, tier: Tier) -> Result<(), u64> {
        self.check_at(ip, tier, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, tier: Tier, now: Instant) -> Result<(), u64> {
        let (burst, rate) = tier.limits();
        let mut buckets = self.buckets.lock().unwrap();
        if buckets.len() > MAX_TRACKED {
            buckets.retain(|_, bucket| now.duration_since(bucket.last) < FORGET_AFTER);
        }
        let bucket = buckets.entry((ip, tier)).or_insert(Bucket {
            tokens: burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - bucket.tokens) / rate).ceil() as u64)
        }
    }

    /// The client's address (see the module documentation).
    pub fn client_ip(&self, request: &Request) -> Option<IpAddr> {
        if let Some(ip) = forwarded_ip(request.headers(), self.hops) {
            return Some(ip);
        }
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip())
    }
}

fn forwarded_ip(headers: &HeaderMap, hops: usize) -> Option<IpAddr> {
    if hops == 0 {
        return None;
    }
    let chain: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    let index = chain.len().checked_sub(hops)?;
    chain[index].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn buckets_empty_then_refill() {
        let limiter = Limiter::new(1);
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let start = Instant::now();
        for _ in 0..20 {
            assert!(limiter.check_at(ip, Tier::Sensitive, start).is_ok());
        }
        let wait = limiter.check_at(ip, Tier::Sensitive, start).unwrap_err();
        assert!((1..=3).contains(&wait));
        // Other clients and the other tier are not affected.
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        assert!(limiter.check_at(other, Tier::Sensitive, start).is_ok());
        assert!(limiter.check_at(ip, Tier::General, start).is_ok());
        // One token comes back every 3 seconds.
        let later = start + Duration::from_secs(3);
        assert!(limiter.check_at(ip, Tier::Sensitive, later).is_ok());
        assert!(limiter.check_at(ip, Tier::Sensitive, later).is_err());
    }

    #[test]
    fn tiers() {
        assert_eq!(Tier::of(&Method::GET, "/dashboard"), Some(Tier::General));
        assert_eq!(
            Tier::of(&Method::POST, "/bots/x/settings"),
            Some(Tier::Sensitive)
        );
        assert_eq!(Tier::of(&Method::GET, "/auth/login"), Some(Tier::Sensitive));
        assert_eq!(
            Tier::of(&Method::GET, "/activity/x/captcha.png"),
            Some(Tier::Verification)
        );
        assert_eq!(
            Tier::of(&Method::POST, "/activity/x/answer"),
            Some(Tier::Verification)
        );
        assert_eq!(Tier::of(&Method::GET, "/activity/x/app.js"), None);
        assert_eq!(Tier::of(&Method::GET, "/assets/app.css"), None);
    }

    #[test]
    fn spoofed_forwarded_entries_are_ignored() {
        let mut headers = HeaderMap::new();
        // The client wrote "1.1.1.1"; the proxy appended the real address.
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.1.1.1, 198.51.100.4"),
        );
        assert_eq!(
            forwarded_ip(&headers, 1),
            Some("198.51.100.4".parse().unwrap())
        );
        assert_eq!(forwarded_ip(&headers, 2), Some("1.1.1.1".parse().unwrap()));
        assert_eq!(forwarded_ip(&headers, 3), None);
        assert_eq!(forwarded_ip(&HeaderMap::new(), 1), None);
    }
}
