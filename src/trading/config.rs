//! Trading configuration.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Configuration for trading and position sizing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    /// Maximum percentage of portfolio to allocate to all positions
    pub max_portfolio_allocation: Decimal,

    /// Maximum percentage of portfolio for a single position
    pub max_single_position: Decimal,

    /// Minimum trade size in USDC
    pub min_trade_size: Decimal,

    /// Maximum trade size in USDC
    pub max_trade_size: Decimal,

    /// Maximum drawdown before stopping trading (0.0 to 1.0)
    pub max_drawdown_pct: Decimal,

    /// Slippage tolerance for market orders (0.0 to 1.0)
    pub slippage_tolerance: Decimal,

    /// Which position sizing method to use
    pub sizing_method: String,

    /// Fraction of Kelly to use (0.0 to 1.0, typically 0.25)
    pub kelly_fraction: Decimal,

    /// Minimum win rate required for a trader to copy
    pub min_win_rate: f64,

    /// Minimum number of trades for a trader to have
    pub min_trades: u32,

    /// Minimum profit in USDC for a trader
    pub min_profit: Decimal,

    /// Maximum acceptable drawdown for a trader (0.0 to 1.0)
    pub max_trader_mdd: f64,

    /// Minimum Sharpe ratio for a trader
    pub min_sharpe: f64,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            max_portfolio_allocation: dec!(0.5),  // Max 50% of capital
            max_single_position: dec!(0.1),       // Max 10% per position
            min_trade_size: dec!(1.0),            // Min $1
            max_trade_size: dec!(1000.0),         // Max $1000
            max_drawdown_pct: dec!(0.2),          // Stop at 20% drawdown
            slippage_tolerance: dec!(0.02),       // 2% slippage
            sizing_method: "kelly".to_string(),
            kelly_fraction: dec!(0.25),           // Quarter Kelly
            min_win_rate: 0.55,
            min_trades: 20,
            min_profit: dec!(100.0),
            max_trader_mdd: 0.4,
            min_sharpe: 0.5,
        }
    }
}
