//! API response types for Polymarket Data API.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Leaderboard entry from /v1/leaderboard endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardEntry {
    pub rank: Option<String>,
    pub proxy_wallet: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub vol: f64,
    #[serde(default)]
    pub pnl: f64,
    #[serde(default)]
    pub profile_image: String,
    #[serde(default)]
    pub x_username: String,
    #[serde(default)]
    pub verified_badge: bool,
}

/// Position response from /positions endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionResponse {
    #[serde(default)]
    pub proxy_wallet: String,
    pub condition_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub slug: String,
    pub outcome: String,
    pub outcome_index: i32,
    pub size: Decimal,
    #[serde(default)]
    pub avg_price: Decimal,
    #[serde(default)]
    pub cur_price: Decimal,
    #[serde(default)]
    pub initial_value: Decimal,
    #[serde(default)]
    pub current_value: Decimal,
    #[serde(default)]
    pub cash_pnl: Decimal,
    #[serde(default)]
    pub percent_pnl: Decimal,
}

/// Trade response from /trades endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeResponse {
    pub proxy_wallet: String,
    pub side: String,
    #[serde(default)]
    pub asset: String,
    pub condition_id: String,
    pub size: Decimal,
    pub price: Decimal,
    pub timestamp: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub outcome_index: i32,
    #[serde(default)]
    pub transaction_hash: String,
    #[serde(default)]
    pub pseudonym: String,
    #[serde(default)]
    pub profile_image: String,
}

/// Activity response from /activity endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityResponse {
    #[serde(rename = "type")]
    pub activity_type: String,
    pub proxy_wallet: String,
    pub condition_id: String,
    #[serde(default)]
    pub size: Decimal,
    #[serde(default)]
    pub usdc_size: Decimal,
    pub timestamp: i64,
    #[serde(default)]
    pub transaction_hash: String,
    #[serde(default)]
    pub side: String,
    #[serde(default)]
    pub outcome: String,
}

/// Portfolio value response from /value endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ValueResponse {
    pub value: Decimal,
}

/// Market holder from /holders endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HolderResponse {
    pub proxy_wallet: String,
    #[serde(default)]
    pub pseudonym: String,
    pub amount: Decimal,
    pub outcome_index: i32,
}

/// Query parameters for various endpoints.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionsParams {
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TradesParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taker_only: Option<bool>,
}
