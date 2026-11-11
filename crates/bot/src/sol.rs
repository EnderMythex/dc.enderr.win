//! "Watching SOL: $123.45" as the bot's activity, when chosen on the
//! dashboard (ported from the Scanline bot). Three sources, tried in order.

use std::{sync::Arc, time::Duration};

use dc_core::settings::BotSettings;
use serde_json::Value;

use crate::{app::App, panels};

const REFRESH: Duration = Duration::from_secs(600);
const SOURCES: [(&str, &str); 3] = [
    (
        "coingecko",
        "https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd",
    ),
    (
        "coinbase",
        "https://api.coinbase.com/v2/prices/SOL-USD/spot",
    ),
    (
        "binance",
        "https://api.binance.com/api/v3/ticker/price?symbol=SOLUSDT",
    ),
];

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

/// The price in one source's answer.
fn price_in(source: &str, data: &Value) -> Option<f64> {
    let price = match source {
        "coingecko" => number(&data["solana"]["usd"]),
        "coinbase" => number(&data["data"]["amount"]),
        _ => number(&data["price"]),
    };
    price.filter(|p| p.is_finite() && *p > 0.0)
}

pub async fn fetch(app: &App) -> Option<f64> {
    for (source, url) in SOURCES {
        let answer = app
            .web
            .get(url)
            .timeout(Duration::from_secs(8))
            .send()
            .await;
        let data: Option<Value> = match answer {
            Ok(response) if response.status().is_success() => response.json().await.ok(),
            _ => None,
        };
        if let Some(price) = data.as_ref().and_then(|data| price_in(source, data)) {
            return Some(price);
        }
        tracing::warn!(source, "SOL price unavailable");
    }
    None
}

pub fn label(price: f64) -> String {
    format!("SOL: ${price:.2}")
}

/// Background task: refreshes the price while the SOL activity is chosen.
pub async fn watch(app: Arc<App>) {
    loop {
        let wanted = BotSettings::load(&app.dir).is_ok_and(|s| s.activity_type == "sol_price");
        if wanted && let Some(price) = fetch(&app).await {
            *app.sol_price.lock().unwrap() = Some(price);
            panels::apply_presence(&app);
        }
        tokio::time::sleep(REFRESH).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prices_from_each_source() {
        assert_eq!(
            price_in("coingecko", &json!({ "solana": { "usd": 151.2 } })),
            Some(151.2)
        );
        assert_eq!(
            price_in("coinbase", &json!({ "data": { "amount": "150.10" } })),
            Some(150.1)
        );
        assert_eq!(
            price_in("binance", &json!({ "price": "149.9" })),
            Some(149.9)
        );
        assert_eq!(price_in("binance", &json!({ "price": "-1" })), None);
        assert_eq!(label(151.234), "SOL: $151.23");
    }
}
