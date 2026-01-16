//! Polymarket Data API client for fetching trader data, positions, and trades.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use std::time::Duration;
use tracing::{debug, warn};

use crate::models::{Position, Trade, TradeSide, Trader};

use super::types::*;

const DATA_API_BASE: &str = "https://data-api.polymarket.com";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Client for Polymarket Data API (read-only operations).
pub struct DataClient {
    client: Client,
    base_url: String,
}

impl DataClient {
    /// Create a new data client with default settings.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url: DATA_API_BASE.to_string(),
        })
    }

    /// Create with custom base URL (for testing).
    pub fn with_base_url(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self { client, base_url })
    }

    /// Fetch trader leaderboard.
    pub async fn get_leaderboard(
        &self,
        category: Option<&str>,
        time_period: Option<&str>,
        order_by: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<LeaderboardEntry>> {
        let mut url = format!("{}/v1/leaderboard", self.base_url);
        let mut params = vec![];

        if let Some(c) = category {
            params.push(format!("category={}", c));
        }
        if let Some(t) = time_period {
            params.push(format!("timePeriod={}", t));
        }
        if let Some(o) = order_by {
            params.push(format!("orderBy={}", o));
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l.min(50)));
        }
        if let Some(o) = offset {
            params.push(format!("offset={}", o));
        }

        if !params.is_empty() {
            url = format!("{}?{}", url, params.join("&"));
        }

        debug!(url = %url, "Fetching leaderboard");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch leaderboard")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Leaderboard request failed: {} - {}", status, body);
        }

        response
            .json()
            .await
            .context("Failed to parse leaderboard response")
    }

    /// Fetch positions for a trader.
    pub async fn get_positions(&self, address: &str, limit: Option<u32>) -> Result<Vec<Position>> {
        let mut url = format!("{}/positions?user={}", self.base_url, address);

        if let Some(l) = limit {
            url = format!("{}&limit={}", url, l.min(500));
        }

        debug!(url = %url, "Fetching positions");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch positions")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Positions request failed: {} - {}", status, body);
        }

        let items: Vec<PositionResponse> = response
            .json()
            .await
            .context("Failed to parse positions response")?;

        let positions = items
            .into_iter()
            .filter_map(|p| {
                Some(Position {
                    trader_address: address.to_string(),
                    market_id: p.condition_id,
                    market_title: p.title,
                    outcome: p.outcome,
                    size: p.size,
                    average_price: p.avg_price,
                    current_price: p.cur_price,
                    initial_value: p.initial_value,
                    current_value: p.current_value,
                    unrealized_pnl: p.cash_pnl,
                    unrealized_pnl_pct: p.percent_pnl,
                    last_updated: Utc::now(),
                })
            })
            .collect();

        Ok(positions)
    }

    /// Fetch trade history for a trader.
    pub async fn get_trades(
        &self,
        address: &str,
        limit: Option<u32>,
        market: Option<&str>,
    ) -> Result<Vec<Trade>> {
        let mut url = format!("{}/trades?user={}&takerOnly=true", self.base_url, address);

        if let Some(l) = limit {
            url = format!("{}&limit={}", url, l.min(500));
        }
        if let Some(m) = market {
            url = format!("{}&market={}", url, m);
        }

        debug!(url = %url, "Fetching trades");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch trades")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Trades request failed: {} - {}", status, body);
        }

        let items: Vec<TradeResponse> = response
            .json()
            .await
            .context("Failed to parse trades response")?;

        let trades = items
            .into_iter()
            .filter_map(|t| {
                let side = match t.side.to_uppercase().as_str() {
                    "BUY" => TradeSide::Buy,
                    "SELL" => TradeSide::Sell,
                    _ => {
                        warn!(side = %t.side, "Unknown trade side");
                        return None;
                    }
                };

                let timestamp = Utc.timestamp_opt(t.timestamp, 0).single()?;

                Some(Trade {
                    id: format!("{}_{}", t.transaction_hash, t.timestamp),
                    trader_address: t.proxy_wallet,
                    market_id: t.condition_id,
                    market_title: t.title,
                    side,
                    outcome: t.outcome,
                    size: t.size,
                    price: t.price,
                    amount_usdc: t.size * t.price,
                    timestamp,
                    transaction_hash: t.transaction_hash,
                    is_taker: true,
                    fee_usdc: Decimal::ZERO,
                })
            })
            .collect();

        Ok(trades)
    }

    /// Fetch portfolio value for a trader.
    pub async fn get_portfolio_value(&self, address: &str) -> Result<Decimal> {
        let url = format!("{}/value?user={}", self.base_url, address);

        debug!(url = %url, "Fetching portfolio value");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch portfolio value")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Value request failed: {} - {}", status, body);
        }

        let value: ValueResponse = response
            .json()
            .await
            .context("Failed to parse value response")?;

        Ok(value.value)
    }

    /// Fetch trader activity (trades, splits, merges, redemptions).
    pub async fn get_activity(
        &self,
        address: &str,
        activity_type: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<ActivityResponse>> {
        let mut url = format!("{}/activity?user={}", self.base_url, address);

        if let Some(t) = activity_type {
            url = format!("{}&type={}", url, t);
        }
        if let Some(l) = limit {
            url = format!("{}&limit={}", url, l.min(500));
        }

        debug!(url = %url, "Fetching activity");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch activity")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Activity request failed: {} - {}", status, body);
        }

        response
            .json()
            .await
            .context("Failed to parse activity response")
    }

    /// Discover top traders from the leaderboard.
    pub async fn discover_top_traders(
        &self,
        min_pnl: f64,
        time_period: &str,
        limit: usize,
    ) -> Result<Vec<Trader>> {
        let mut traders = Vec::new();
        let mut offset = 0u32;
        let page_size = 50u32;

        while traders.len() < limit {
            let entries = self
                .get_leaderboard(
                    Some("OVERALL"),
                    Some(time_period),
                    Some("PNL"),
                    Some(page_size),
                    Some(offset),
                )
                .await?;

            if entries.is_empty() {
                break;
            }

            for entry in entries {
                if entry.pnl >= min_pnl {
                    traders.push(Trader {
                        address: entry.proxy_wallet,
                        pseudonym: entry.user_name,
                        profile_image: entry.profile_image,
                        bio: String::new(),
                        is_tracked: false,
                        tracking_since: None,
                        positions: Vec::new(),
                        metrics: None,
                        allocation_weight: Decimal::ONE,
                    });
                }

                if traders.len() >= limit {
                    break;
                }
            }

            offset += page_size;

            // Rate limiting
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(traders)
    }
}

impl Default for DataClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default DataClient")
    }
}
