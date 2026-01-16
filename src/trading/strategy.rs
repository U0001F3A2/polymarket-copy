//! Trading strategy with entry/exit rules and risk management.
//!
//! This module defines the rules for:
//! - When to enter positions (copy trade validation)
//! - When to exit positions (profit targets, stop losses, time-based)
//! - Portfolio-level risk management

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::models::TraderMetrics;

/// Trading strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    // === Entry Rules ===
    /// Maximum age of a trade to copy (seconds)
    pub max_trade_age_secs: i64,

    /// Minimum price (0-1) to enter a position
    pub min_entry_price: Decimal,

    /// Maximum price (0-1) to enter a position
    pub max_entry_price: Decimal,

    /// Maximum slippage from source trade price
    pub max_entry_slippage: Decimal,

    /// Minimum trader composite score (0-100)
    pub min_trader_score: f64,

    /// Only copy trades if trader is in profit overall
    pub require_profitable_trader: bool,

    /// Only copy trades in markets with sufficient liquidity
    pub min_market_liquidity: Decimal,

    // === Exit Rules ===
    /// Take profit percentage (e.g., 0.2 = 20% profit)
    pub take_profit_pct: Decimal,

    /// Stop loss percentage (e.g., 0.1 = 10% loss)
    pub stop_loss_pct: Decimal,

    /// Maximum holding period in hours
    pub max_holding_hours: i64,

    /// Exit if source trader exits
    pub follow_trader_exits: bool,

    /// Exit positions that approach market resolution
    pub exit_before_resolution_hours: i64,

    // === Portfolio Risk ===
    /// Maximum portfolio drawdown before halting (0-1)
    pub max_portfolio_drawdown: Decimal,

    /// Maximum number of concurrent positions
    pub max_concurrent_positions: usize,

    /// Maximum exposure to single market
    pub max_single_market_exposure: Decimal,

    /// Minimum time between trades (anti-churn)
    pub min_trade_interval_secs: i64,

    /// Cool-off period after a losing trade (seconds)
    pub loss_cooloff_secs: i64,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            // Entry rules
            max_trade_age_secs: 300,          // 5 minutes
            min_entry_price: dec!(0.05),      // Don't buy below 5%
            max_entry_price: dec!(0.95),      // Don't buy above 95%
            max_entry_slippage: dec!(0.03),   // 3% slippage tolerance
            min_trader_score: 40.0,           // Minimum composite score
            require_profitable_trader: true,
            min_market_liquidity: dec!(1000), // $1000 min liquidity

            // Exit rules
            take_profit_pct: dec!(0.25),      // 25% profit target
            stop_loss_pct: dec!(0.15),        // 15% stop loss
            max_holding_hours: 168,           // 7 days max hold
            follow_trader_exits: true,
            exit_before_resolution_hours: 24, // Exit 24h before resolution

            // Portfolio risk
            max_portfolio_drawdown: dec!(0.20),  // 20% max DD
            max_concurrent_positions: 10,
            max_single_market_exposure: dec!(0.25), // 25% max in one market
            min_trade_interval_secs: 60,         // 1 min between trades
            loss_cooloff_secs: 300,              // 5 min after loss
        }
    }
}

/// Represents a position for strategy evaluation.
#[derive(Debug, Clone)]
pub struct StrategyPosition {
    pub market_id: String,
    pub outcome: String,
    pub side: String,
    pub entry_price: Decimal,
    pub current_price: Decimal,
    pub size: Decimal,
    pub unrealized_pnl: Decimal,
    pub opened_at: DateTime<Utc>,
    pub source_trader: Option<String>,
}

impl StrategyPosition {
    /// Calculate return percentage.
    pub fn return_pct(&self) -> Decimal {
        if self.entry_price.is_zero() {
            return Decimal::ZERO;
        }
        (self.current_price - self.entry_price) / self.entry_price
    }

    /// Check if position is profitable.
    pub fn is_profitable(&self) -> bool {
        self.unrealized_pnl > Decimal::ZERO
    }

    /// Get holding duration.
    pub fn holding_duration(&self) -> Duration {
        Utc::now() - self.opened_at
    }
}

/// Result of entry validation.
#[derive(Debug, Clone)]
pub struct EntryValidation {
    pub allowed: bool,
    pub reason: String,
    pub adjusted_size: Option<Decimal>,
}

impl EntryValidation {
    pub fn allow(size: Decimal) -> Self {
        Self {
            allowed: true,
            reason: "Entry conditions met".to_string(),
            adjusted_size: Some(size),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            adjusted_size: None,
        }
    }
}

/// Exit signal with reason.
#[derive(Debug, Clone)]
pub struct ExitSignal {
    pub should_exit: bool,
    pub reason: ExitReason,
    pub urgency: ExitUrgency,
}

/// Reason for exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    TakeProfit,
    StopLoss,
    MaxHoldingPeriod,
    TraderExited,
    MarketResolution,
    PortfolioRisk,
    ManualClose,
    None,
}

/// How urgently to exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitUrgency {
    /// Exit immediately with market order
    Immediate,
    /// Exit soon, can use limit order
    Normal,
    /// Exit when convenient
    Low,
    /// No exit needed
    None,
}

/// Portfolio state for risk evaluation.
#[derive(Debug, Clone)]
pub struct PortfolioState {
    pub total_value: Decimal,
    pub cash_available: Decimal,
    pub total_exposure: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub current_drawdown: Decimal,
    pub position_count: usize,
    pub last_trade_at: Option<DateTime<Utc>>,
    pub last_loss_at: Option<DateTime<Utc>>,
}

/// Trading strategy engine.
pub struct Strategy {
    config: StrategyConfig,
}

impl Strategy {
    /// Create a new strategy with configuration.
    pub fn new(config: StrategyConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration.
    pub fn default_strategy() -> Self {
        Self::new(StrategyConfig::default())
    }

    /// Get strategy configuration.
    pub fn config(&self) -> &StrategyConfig {
        &self.config
    }

    // ==================== Entry Validation ====================

    /// Validate whether we should enter a position.
    ///
    /// `reference_time` is used for calculating trade age. Pass `None` for live trading
    /// (uses current time), or pass a simulated time for backtesting.
    pub fn validate_entry(
        &self,
        source_trade_time: DateTime<Utc>,
        current_price: Decimal,
        source_price: Decimal,
        proposed_size: Decimal,
        trader_metrics: Option<&TraderMetrics>,
        portfolio: &PortfolioState,
        market_positions: &[StrategyPosition],
        reference_time: Option<DateTime<Utc>>,
    ) -> EntryValidation {
        // Check trade age (skip for backtesting when reference_time equals trade time)
        let now = reference_time.unwrap_or_else(Utc::now);
        let trade_age = now - source_trade_time;
        if trade_age.num_seconds() > self.config.max_trade_age_secs {
            return EntryValidation::deny(format!(
                "Trade too old: {}s > {}s",
                trade_age.num_seconds(),
                self.config.max_trade_age_secs
            ));
        }

        // Check price bounds
        if current_price < self.config.min_entry_price {
            return EntryValidation::deny(format!(
                "Price too low: {} < {}",
                current_price, self.config.min_entry_price
            ));
        }
        if current_price > self.config.max_entry_price {
            return EntryValidation::deny(format!(
                "Price too high: {} > {}",
                current_price, self.config.max_entry_price
            ));
        }

        // Check slippage from source trade
        let slippage = if source_price > Decimal::ZERO {
            ((current_price - source_price) / source_price).abs()
        } else {
            Decimal::ZERO
        };
        if slippage > self.config.max_entry_slippage {
            return EntryValidation::deny(format!(
                "Slippage too high: {}% > {}%",
                slippage * dec!(100),
                self.config.max_entry_slippage * dec!(100)
            ));
        }

        // Check trader quality
        if let Some(metrics) = trader_metrics {
            let score = metrics.composite_score();
            if score < self.config.min_trader_score {
                return EntryValidation::deny(format!(
                    "Trader score too low: {:.1} < {}",
                    score, self.config.min_trader_score
                ));
            }

            if self.config.require_profitable_trader && metrics.total_pnl <= Decimal::ZERO {
                return EntryValidation::deny("Trader not profitable overall");
            }
        }

        // Check portfolio constraints
        if let Some(validation) = self.check_portfolio_constraints(portfolio, proposed_size) {
            return validation;
        }

        // Check market exposure
        let market_exposure: Decimal = market_positions.iter().map(|p| p.size).sum();
        let max_market = portfolio.total_value * self.config.max_single_market_exposure;
        if market_exposure + proposed_size > max_market {
            let allowed_size = (max_market - market_exposure).max(Decimal::ZERO);
            if allowed_size < dec!(1) {
                return EntryValidation::deny(format!(
                    "Market exposure limit: {} + {} > {}",
                    market_exposure, proposed_size, max_market
                ));
            }
            info!(
                proposed = %proposed_size,
                allowed = %allowed_size,
                "Reducing size due to market exposure limit"
            );
            return EntryValidation::allow(allowed_size);
        }

        // Check trade interval (anti-churn)
        if let Some(last_trade) = portfolio.last_trade_at {
            let since_last = (Utc::now() - last_trade).num_seconds();
            if since_last < self.config.min_trade_interval_secs {
                return EntryValidation::deny(format!(
                    "Too soon after last trade: {}s < {}s",
                    since_last, self.config.min_trade_interval_secs
                ));
            }
        }

        // Check loss cool-off
        if let Some(last_loss) = portfolio.last_loss_at {
            let since_loss = (Utc::now() - last_loss).num_seconds();
            if since_loss < self.config.loss_cooloff_secs {
                return EntryValidation::deny(format!(
                    "In loss cool-off period: {}s remaining",
                    self.config.loss_cooloff_secs - since_loss
                ));
            }
        }

        EntryValidation::allow(proposed_size)
    }

    /// Check portfolio-level constraints.
    fn check_portfolio_constraints(
        &self,
        portfolio: &PortfolioState,
        proposed_size: Decimal,
    ) -> Option<EntryValidation> {
        // Check drawdown
        if portfolio.current_drawdown >= self.config.max_portfolio_drawdown {
            return Some(EntryValidation::deny(format!(
                "Portfolio drawdown too high: {}% >= {}%",
                portfolio.current_drawdown * dec!(100),
                self.config.max_portfolio_drawdown * dec!(100)
            )));
        }

        // Check position count
        if portfolio.position_count >= self.config.max_concurrent_positions {
            return Some(EntryValidation::deny(format!(
                "Too many positions: {} >= {}",
                portfolio.position_count, self.config.max_concurrent_positions
            )));
        }

        // Check available cash
        if proposed_size > portfolio.cash_available {
            if portfolio.cash_available < dec!(1) {
                return Some(EntryValidation::deny("Insufficient cash"));
            }
            return Some(EntryValidation::allow(portfolio.cash_available));
        }

        None
    }

    // ==================== Exit Signals ====================

    /// Check if a position should be exited.
    pub fn check_exit(
        &self,
        position: &StrategyPosition,
        portfolio: &PortfolioState,
        trader_still_holding: bool,
        market_resolution_time: Option<DateTime<Utc>>,
    ) -> ExitSignal {
        // Check take profit
        let return_pct = position.return_pct();
        if return_pct >= self.config.take_profit_pct {
            debug!(
                market = %position.market_id,
                return_pct = %return_pct,
                target = %self.config.take_profit_pct,
                "Take profit triggered"
            );
            return ExitSignal {
                should_exit: true,
                reason: ExitReason::TakeProfit,
                urgency: ExitUrgency::Normal,
            };
        }

        // Check stop loss
        if return_pct <= -self.config.stop_loss_pct {
            warn!(
                market = %position.market_id,
                return_pct = %return_pct,
                stop = %self.config.stop_loss_pct,
                "Stop loss triggered"
            );
            return ExitSignal {
                should_exit: true,
                reason: ExitReason::StopLoss,
                urgency: ExitUrgency::Immediate,
            };
        }

        // Check max holding period
        let holding_hours = position.holding_duration().num_hours();
        if holding_hours >= self.config.max_holding_hours {
            info!(
                market = %position.market_id,
                hours = holding_hours,
                max = self.config.max_holding_hours,
                "Max holding period reached"
            );
            return ExitSignal {
                should_exit: true,
                reason: ExitReason::MaxHoldingPeriod,
                urgency: ExitUrgency::Normal,
            };
        }

        // Check if source trader exited
        if self.config.follow_trader_exits && !trader_still_holding {
            info!(
                market = %position.market_id,
                trader = ?position.source_trader,
                "Source trader exited position"
            );
            return ExitSignal {
                should_exit: true,
                reason: ExitReason::TraderExited,
                urgency: ExitUrgency::Normal,
            };
        }

        // Check market resolution proximity
        if let Some(resolution_time) = market_resolution_time {
            let hours_to_resolution = (resolution_time - Utc::now()).num_hours();
            if hours_to_resolution <= self.config.exit_before_resolution_hours
                && hours_to_resolution > 0
            {
                info!(
                    market = %position.market_id,
                    hours_remaining = hours_to_resolution,
                    "Approaching market resolution"
                );
                return ExitSignal {
                    should_exit: true,
                    reason: ExitReason::MarketResolution,
                    urgency: ExitUrgency::Normal,
                };
            }
        }

        // Check portfolio risk
        if portfolio.current_drawdown >= self.config.max_portfolio_drawdown {
            warn!(
                drawdown = %portfolio.current_drawdown,
                "Portfolio drawdown limit hit, closing positions"
            );
            return ExitSignal {
                should_exit: true,
                reason: ExitReason::PortfolioRisk,
                urgency: ExitUrgency::Immediate,
            };
        }

        ExitSignal {
            should_exit: false,
            reason: ExitReason::None,
            urgency: ExitUrgency::None,
        }
    }

    /// Evaluate all positions and return those that should be exited.
    pub fn evaluate_exits(
        &self,
        positions: &[StrategyPosition],
        portfolio: &PortfolioState,
        trader_holdings: &std::collections::HashMap<String, Vec<String>>, // trader -> market_ids
    ) -> Vec<(StrategyPosition, ExitSignal)> {
        positions
            .iter()
            .filter_map(|pos| {
                // Check if source trader still holds
                let trader_holding = pos.source_trader.as_ref().map_or(true, |trader| {
                    trader_holdings
                        .get(trader)
                        .map_or(false, |markets| markets.contains(&pos.market_id))
                });

                let signal = self.check_exit(pos, portfolio, trader_holding, None);
                if signal.should_exit {
                    Some((pos.clone(), signal))
                } else {
                    None
                }
            })
            .collect()
    }

    // ==================== Risk Management ====================

    /// Check if trading should be halted due to portfolio risk.
    pub fn should_halt_trading(&self, portfolio: &PortfolioState) -> (bool, String) {
        if portfolio.current_drawdown >= self.config.max_portfolio_drawdown {
            return (
                true,
                format!(
                    "Portfolio drawdown {}% exceeds limit {}%",
                    (portfolio.current_drawdown * dec!(100)).round(),
                    (self.config.max_portfolio_drawdown * dec!(100)).round()
                ),
            );
        }

        // Additional halt conditions could be added here
        // e.g., excessive losses in short period, API errors, etc.

        (false, String::new())
    }

    /// Calculate position-level risk metrics.
    pub fn calculate_position_risk(&self, position: &StrategyPosition) -> PositionRisk {
        let return_pct = position.return_pct();
        let distance_to_stop = return_pct + self.config.stop_loss_pct;
        let distance_to_target = self.config.take_profit_pct - return_pct;

        let risk_score = if distance_to_stop <= Decimal::ZERO {
            1.0 // At or past stop loss
        } else if distance_to_stop < dec!(0.05) {
            0.8 // Within 5% of stop
        } else if return_pct < Decimal::ZERO {
            0.5 // In loss but above stop
        } else {
            0.2 // In profit
        };

        PositionRisk {
            return_pct,
            distance_to_stop,
            distance_to_target,
            holding_hours: position.holding_duration().num_hours(),
            risk_score,
        }
    }
}

/// Risk metrics for a position.
#[derive(Debug, Clone)]
pub struct PositionRisk {
    pub return_pct: Decimal,
    pub distance_to_stop: Decimal,
    pub distance_to_target: Decimal,
    pub holding_hours: i64,
    pub risk_score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_position(entry: Decimal, current: Decimal, hours_ago: i64) -> StrategyPosition {
        StrategyPosition {
            market_id: "test-market".to_string(),
            outcome: "Yes".to_string(),
            side: "BUY".to_string(),
            entry_price: entry,
            current_price: current,
            size: dec!(100),
            unrealized_pnl: (current - entry) * dec!(100),
            opened_at: Utc::now() - Duration::hours(hours_ago),
            source_trader: Some("0x123".to_string()),
        }
    }

    fn make_portfolio() -> PortfolioState {
        PortfolioState {
            total_value: dec!(10000),
            cash_available: dec!(5000),
            total_exposure: dec!(5000),
            unrealized_pnl: dec!(200),
            realized_pnl: dec!(100),
            current_drawdown: dec!(0.05),
            position_count: 3,
            last_trade_at: None,
            last_loss_at: None,
        }
    }

    #[test]
    fn test_take_profit() {
        let strategy = Strategy::default_strategy();
        let position = make_position(dec!(0.50), dec!(0.65), 24); // 30% profit
        let portfolio = make_portfolio();

        let signal = strategy.check_exit(&position, &portfolio, true, None);
        assert!(signal.should_exit);
        assert_eq!(signal.reason, ExitReason::TakeProfit);
    }

    #[test]
    fn test_stop_loss() {
        let strategy = Strategy::default_strategy();
        let position = make_position(dec!(0.50), dec!(0.40), 24); // 20% loss
        let portfolio = make_portfolio();

        let signal = strategy.check_exit(&position, &portfolio, true, None);
        assert!(signal.should_exit);
        assert_eq!(signal.reason, ExitReason::StopLoss);
        assert_eq!(signal.urgency, ExitUrgency::Immediate);
    }

    #[test]
    fn test_max_holding_period() {
        let strategy = Strategy::default_strategy();
        let position = make_position(dec!(0.50), dec!(0.52), 200); // 200 hours
        let portfolio = make_portfolio();

        let signal = strategy.check_exit(&position, &portfolio, true, None);
        assert!(signal.should_exit);
        assert_eq!(signal.reason, ExitReason::MaxHoldingPeriod);
    }

    #[test]
    fn test_entry_validation_price_bounds() {
        let strategy = Strategy::default_strategy();
        let portfolio = make_portfolio();

        // Price too low
        let result = strategy.validate_entry(
            Utc::now(),
            dec!(0.02), // Too low
            dec!(0.02),
            dec!(100),
            None,
            &portfolio,
            &[],
            None,
        );
        assert!(!result.allowed);
        assert!(result.reason.contains("too low"));

        // Price too high
        let result = strategy.validate_entry(
            Utc::now(),
            dec!(0.98), // Too high
            dec!(0.98),
            dec!(100),
            None,
            &portfolio,
            &[],
            None,
        );
        assert!(!result.allowed);
        assert!(result.reason.contains("too high"));

        // Price OK
        let result = strategy.validate_entry(
            Utc::now(),
            dec!(0.50),
            dec!(0.50),
            dec!(100),
            None,
            &portfolio,
            &[],
            None,
        );
        assert!(result.allowed);
    }

    #[test]
    fn test_entry_validation_trade_age() {
        let strategy = Strategy::default_strategy();
        let portfolio = make_portfolio();

        // Trade too old
        let result = strategy.validate_entry(
            Utc::now() - Duration::minutes(10), // 10 minutes ago
            dec!(0.50),
            dec!(0.50),
            dec!(100),
            None,
            &portfolio,
            &[],
            None,
        );
        assert!(!result.allowed);
        assert!(result.reason.contains("too old"));
    }
}
