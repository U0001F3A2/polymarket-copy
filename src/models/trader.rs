//! Trader model representing a Polymarket trader profile.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::metrics::TraderMetrics;
use super::position::Position;

/// Trader profile with metrics and tracking status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trader {
    /// Wallet address (0x-prefixed)
    pub address: String,

    /// Display name / pseudonym
    #[serde(default)]
    pub pseudonym: String,

    /// Profile image URL
    #[serde(default)]
    pub profile_image: String,

    /// Bio/description
    #[serde(default)]
    pub bio: String,

    /// Whether we're actively tracking this trader
    #[serde(default)]
    pub is_tracked: bool,

    /// When we started tracking this trader
    pub tracking_since: Option<DateTime<Utc>>,

    /// Current open positions
    #[serde(default)]
    pub positions: Vec<Position>,

    /// Calculated performance metrics
    pub metrics: Option<TraderMetrics>,

    /// Relative weight for position sizing (1.0 = 100%)
    #[serde(default = "default_weight")]
    pub allocation_weight: Decimal,
}

fn default_weight() -> Decimal {
    Decimal::ONE
}

impl Trader {
    /// Create a new trader from address.
    pub fn new(address: String) -> Self {
        Self {
            address,
            pseudonym: String::new(),
            profile_image: String::new(),
            bio: String::new(),
            is_tracked: false,
            tracking_since: None,
            positions: Vec::new(),
            metrics: None,
            allocation_weight: Decimal::ONE,
        }
    }

    /// Get display name (pseudonym or truncated address).
    pub fn display_name(&self) -> String {
        if !self.pseudonym.is_empty() {
            self.pseudonym.clone()
        } else if self.address.len() > 10 {
            format!("{}...{}", &self.address[..6], &self.address[self.address.len() - 4..])
        } else {
            self.address.clone()
        }
    }

    /// Start tracking this trader.
    pub fn start_tracking(&mut self) {
        self.is_tracked = true;
        self.tracking_since = Some(Utc::now());
    }

    /// Stop tracking this trader.
    pub fn stop_tracking(&mut self) {
        self.is_tracked = false;
    }

    /// Get trader's composite score for ranking.
    pub fn score(&self) -> f64 {
        self.metrics.as_ref().map(|m| m.composite_score()).unwrap_or(0.0)
    }

    /// Check if trader meets minimum requirements for copying.
    pub fn meets_requirements(
        &self,
        min_win_rate: f64,
        min_trades: u32,
        min_profit: Decimal,
        max_mdd: f64,
        min_sharpe: f64,
    ) -> bool {
        let Some(metrics) = &self.metrics else {
            return false;
        };

        metrics.win_rate >= min_win_rate
            && metrics.total_trades >= min_trades
            && metrics.total_pnl >= min_profit
            && metrics.max_drawdown <= max_mdd
            && metrics.sharpe_ratio >= min_sharpe
    }

    /// Total value of all open positions.
    pub fn total_position_value(&self) -> Decimal {
        self.positions.iter().map(|p| p.current_value).sum()
    }
}

impl Default for Trader {
    fn default() -> Self {
        Self::new(String::new())
    }
}
