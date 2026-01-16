//! Trader performance metrics including MDD, Sharpe ratio, win rate, etc.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Comprehensive performance metrics for a trader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraderMetrics {
    /// Trader's address
    pub address: String,

    /// When these metrics were calculated
    pub calculated_at: DateTime<Utc>,

    // === Basic Statistics ===
    /// Total number of trades
    pub total_trades: u32,

    /// Total trading volume in USDC
    pub total_volume: Decimal,

    /// Total realized P&L in USDC
    pub total_pnl: Decimal,

    // === Win/Loss Metrics ===
    /// Number of winning trades
    pub winning_trades: u32,

    /// Number of losing trades
    pub losing_trades: u32,

    /// Win rate (0.0 to 1.0)
    pub win_rate: f64,

    /// Average profit on winning trades
    pub avg_win: Decimal,

    /// Average loss on losing trades (absolute value)
    pub avg_loss: Decimal,

    /// Profit factor (gross profit / gross loss)
    pub profit_factor: f64,

    /// Expectancy per trade in USDC
    pub expectancy: Decimal,

    // === Risk Metrics ===
    /// Maximum drawdown percentage (0.0 to 1.0)
    pub max_drawdown: f64,

    /// Maximum drawdown in absolute USDC terms
    pub max_drawdown_usdc: Decimal,

    /// Peak equity (for drawdown calculation)
    pub peak_equity: Decimal,

    /// Annualized Sharpe ratio (risk-adjusted returns)
    pub sharpe_ratio: f64,

    /// Sortino ratio (downside risk-adjusted returns)
    pub sortino_ratio: f64,

    /// Calmar ratio (return / max drawdown)
    pub calmar_ratio: f64,

    // === Time-Based Metrics ===
    /// Average holding period in hours
    pub avg_holding_period_hours: f64,

    /// Number of trades per day (activity level)
    pub trades_per_day: f64,

    // === Recent Performance (trend detection) ===
    /// P&L in last 7 days
    pub pnl_7d: Decimal,

    /// P&L in last 30 days
    pub pnl_30d: Decimal,

    /// Win rate in last 7 days
    pub win_rate_7d: f64,

    /// Win rate in last 30 days
    pub win_rate_30d: f64,
}

impl TraderMetrics {
    /// Create empty metrics for an address.
    pub fn new(address: String) -> Self {
        Self {
            address,
            calculated_at: Utc::now(),
            total_trades: 0,
            total_volume: Decimal::ZERO,
            total_pnl: Decimal::ZERO,
            winning_trades: 0,
            losing_trades: 0,
            win_rate: 0.0,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 0.0,
            expectancy: Decimal::ZERO,
            max_drawdown: 0.0,
            max_drawdown_usdc: Decimal::ZERO,
            peak_equity: Decimal::ZERO,
            sharpe_ratio: 0.0,
            sortino_ratio: 0.0,
            calmar_ratio: 0.0,
            avg_holding_period_hours: 0.0,
            trades_per_day: 0.0,
            pnl_7d: Decimal::ZERO,
            pnl_30d: Decimal::ZERO,
            win_rate_7d: 0.0,
            win_rate_30d: 0.0,
        }
    }

    /// Calculate composite score for trader ranking (0-100).
    ///
    /// Weights:
    /// - Win rate: 25%
    /// - Sharpe ratio: 25%
    /// - Low drawdown: 25%
    /// - Profitability: 15%
    /// - Recent momentum: 10%
    pub fn composite_score(&self) -> f64 {
        if self.total_trades < 10 {
            return 0.0;
        }

        // Win rate score (0-25 points, >60% gets full score)
        let win_rate_score = (self.win_rate / 0.6).min(1.0) * 25.0;

        // Sharpe ratio score (0-25 points, Sharpe of 2+ gets full score)
        let sharpe_score = (self.sharpe_ratio / 2.0).min(1.0).max(0.0) * 25.0;

        // Low drawdown score (0-25 points, <10% MDD gets full score)
        let drawdown_score = (1.0 - self.max_drawdown / 0.5).max(0.0).min(1.0) * 25.0;

        // Profitability score (0-15 points)
        let pnl_f64: f64 = self.total_pnl.try_into().unwrap_or(0.0);
        let profit_score = (pnl_f64 / 5000.0).min(1.0).max(0.0) * 15.0;

        // Recent momentum score (0-10 points)
        let pnl_7d_f64: f64 = self.pnl_7d.try_into().unwrap_or(0.0);
        let momentum_score = if pnl_7d_f64 > 0.0 {
            (pnl_7d_f64 / 500.0).min(1.0) * 10.0
        } else {
            0.0
        };

        win_rate_score + sharpe_score + drawdown_score + profit_score + momentum_score
    }

    /// Suggested position sizing multiplier based on metrics (0.0 to 1.0).
    ///
    /// Uses a modified Kelly criterion capped for safety.
    pub fn suggested_allocation(&self) -> f64 {
        if self.total_trades < 10 || self.win_rate < 0.5 {
            return 0.0;
        }

        // Kelly criterion: f* = (bp - q) / b
        // where b = avg_win/avg_loss, p = win_rate, q = 1 - p
        let avg_win_f64: f64 = self.avg_win.try_into().unwrap_or(0.0);
        let avg_loss_f64: f64 = self.avg_loss.try_into().unwrap_or(1.0);

        if avg_loss_f64 <= 0.0 {
            return 0.0;
        }

        let b = avg_win_f64 / avg_loss_f64;
        let p = self.win_rate;
        let q = 1.0 - p;

        let kelly = (b * p - q) / b;

        // Cap at 25% Kelly for safety and apply drawdown penalty
        let capped_kelly = kelly.max(0.0).min(0.25);
        let drawdown_penalty = 1.0 - self.max_drawdown;

        capped_kelly * drawdown_penalty
    }

    /// Check if metrics indicate a trader worth following.
    pub fn is_quality_trader(&self) -> bool {
        self.total_trades >= 20
            && self.win_rate >= 0.52
            && self.sharpe_ratio >= 0.3
            && self.max_drawdown <= 0.5
            && self.total_pnl > Decimal::ZERO
    }
}

impl Default for TraderMetrics {
    fn default() -> Self {
        Self::new(String::new())
    }
}
