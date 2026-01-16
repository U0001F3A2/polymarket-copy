//! Market model representing a Polymarket prediction market.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Market resolution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MarketStatus {
    #[default]
    Active,
    Resolved,
    Cancelled,
}

/// Prediction market information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// Unique market condition ID (0x-prefixed)
    pub condition_id: String,

    /// Human-readable title
    pub title: String,

    /// URL-friendly slug
    pub slug: String,

    /// Detailed description
    #[serde(default)]
    pub description: String,

    /// Category (e.g., "politics", "sports", "crypto")
    #[serde(default)]
    pub category: String,

    /// When the market ends
    pub end_date: Option<DateTime<Utc>>,

    /// Current market status
    #[serde(default)]
    pub status: MarketStatus,

    /// Winning outcome if resolved
    pub winning_outcome: Option<String>,

    /// Mapping of outcome name to token ID
    #[serde(default)]
    pub tokens: HashMap<String, String>,

    /// Current prices per outcome (0.0 to 1.0)
    #[serde(default)]
    pub prices: HashMap<String, Decimal>,

    /// 24h trading volume in USDC
    #[serde(default)]
    pub volume_24h: Decimal,

    /// Total liquidity in USDC
    #[serde(default)]
    pub liquidity: Decimal,

    /// Last updated timestamp
    #[serde(default = "Utc::now")]
    pub last_updated: DateTime<Utc>,
}

impl Market {
    /// Create a new market with minimal info.
    pub fn new(condition_id: String, title: String) -> Self {
        Self {
            condition_id,
            title,
            slug: String::new(),
            description: String::new(),
            category: String::new(),
            end_date: None,
            status: MarketStatus::Active,
            winning_outcome: None,
            tokens: HashMap::new(),
            prices: HashMap::new(),
            volume_24h: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            last_updated: Utc::now(),
        }
    }

    /// Check if market is still tradeable.
    pub fn is_active(&self) -> bool {
        self.status == MarketStatus::Active
    }

    /// Check if market has been resolved.
    pub fn is_resolved(&self) -> bool {
        self.status == MarketStatus::Resolved
    }

    /// Get price for a specific outcome.
    pub fn price_for(&self, outcome: &str) -> Option<Decimal> {
        self.prices.get(outcome).copied()
    }

    /// Get the spread between best bid and ask (for binary markets).
    pub fn spread(&self) -> Option<Decimal> {
        let yes_price = self.prices.get("Yes")?;
        let no_price = self.prices.get("No")?;

        // In a proper binary market, Yes + No should ≈ 1.0
        // Spread is how much they deviate
        Some((yes_price + no_price - Decimal::ONE).abs())
    }

    /// Check if this is a binary (Yes/No) market.
    pub fn is_binary(&self) -> bool {
        self.tokens.len() == 2 && self.tokens.contains_key("Yes") && self.tokens.contains_key("No")
    }
}

impl Default for Market {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}
