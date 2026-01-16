//! Position sizing algorithms: Kelly criterion, fixed fraction, risk parity.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

use crate::models::TraderMetrics;
use super::TradingConfig;

/// Position sizing method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingMethod {
    /// Kelly criterion (fraction based on edge and odds)
    Kelly,
    /// Fixed percentage of capital
    FixedFraction,
    /// Allocate based on inverse volatility
    RiskParity,
    /// Simple equal allocation
    Equal,
}

impl SizingMethod {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "kelly" => Self::Kelly,
            "fixed" | "fixed_fraction" => Self::FixedFraction,
            "risk_parity" | "riskparity" => Self::RiskParity,
            _ => Self::Equal,
        }
    }
}

/// Calculator for optimal position sizes.
pub struct PositionSizer {
    config: TradingConfig,
    method: SizingMethod,
}

impl PositionSizer {
    /// Create a new position sizer with given config.
    pub fn new(config: TradingConfig) -> Self {
        let method = SizingMethod::from_str(&config.sizing_method);
        Self { config, method }
    }

    /// Calculate the position size for copying a trade.
    ///
    /// # Arguments
    /// * `source_trade_size` - Size of the trade being copied (in USDC)
    /// * `source_portfolio_value` - Total portfolio value of the trader being copied
    /// * `our_portfolio_value` - Our total portfolio value
    /// * `trader_metrics` - Performance metrics of the trader
    /// * `current_exposure` - Our current total exposure in USDC
    ///
    /// # Returns
    /// Recommended position size in USDC
    pub fn calculate_size(
        &self,
        source_trade_size: Decimal,
        source_portfolio_value: Decimal,
        our_portfolio_value: Decimal,
        trader_metrics: Option<&TraderMetrics>,
        current_exposure: Decimal,
    ) -> Decimal {
        // Base multiplier from portfolio ratio
        let base_multiplier = if source_portfolio_value > Decimal::ZERO {
            our_portfolio_value / source_portfolio_value
        } else {
            Decimal::ONE
        };

        // Calculate raw position size
        let raw_size = source_trade_size * base_multiplier;

        // Apply sizing method
        let sized = match self.method {
            SizingMethod::Kelly => self.kelly_size(raw_size, trader_metrics, our_portfolio_value),
            SizingMethod::FixedFraction => self.fixed_fraction_size(our_portfolio_value),
            SizingMethod::RiskParity => {
                self.risk_parity_size(raw_size, trader_metrics, our_portfolio_value)
            }
            SizingMethod::Equal => raw_size,
        };

        // Apply constraints
        self.apply_constraints(sized, our_portfolio_value, current_exposure)
    }

    /// Kelly criterion position sizing.
    ///
    /// f* = (p * b - q) / b
    /// where:
    ///   p = probability of winning (win rate)
    ///   q = probability of losing (1 - p)
    ///   b = ratio of average win to average loss
    fn kelly_size(
        &self,
        base_size: Decimal,
        metrics: Option<&TraderMetrics>,
        portfolio_value: Decimal,
    ) -> Decimal {
        let Some(m) = metrics else {
            return base_size * dec!(0.1); // Conservative if no metrics
        };

        if m.win_rate < 0.5 || m.avg_loss.is_zero() {
            return Decimal::ZERO; // No edge, don't bet
        }

        let p = m.win_rate;
        let q = 1.0 - p;
        let b = m.avg_win.to_f64().unwrap_or(1.0) / m.avg_loss.to_f64().unwrap_or(1.0);

        let kelly = (p * b - q) / b;

        if kelly <= 0.0 {
            return Decimal::ZERO;
        }

        // Apply Kelly fraction (e.g., 0.25 for quarter Kelly)
        let adjusted_kelly = kelly * self.config.kelly_fraction.to_f64().unwrap_or(0.25);

        // Apply drawdown penalty
        let drawdown_penalty = 1.0 - m.max_drawdown.min(0.9);
        let final_kelly = adjusted_kelly * drawdown_penalty;

        // Convert to position size
        let kelly_size = portfolio_value * Decimal::try_from(final_kelly).unwrap_or(dec!(0.1));

        // Take minimum of Kelly-based size and base size
        kelly_size.min(base_size)
    }

    /// Fixed fraction position sizing.
    fn fixed_fraction_size(&self, portfolio_value: Decimal) -> Decimal {
        portfolio_value * self.config.max_single_position
    }

    /// Risk parity: size inversely proportional to volatility/drawdown.
    fn risk_parity_size(
        &self,
        base_size: Decimal,
        metrics: Option<&TraderMetrics>,
        portfolio_value: Decimal,
    ) -> Decimal {
        let Some(m) = metrics else {
            return base_size * dec!(0.5);
        };

        // Use max drawdown as volatility proxy
        let volatility = m.max_drawdown.max(0.1);

        // Target volatility contribution (e.g., 10%)
        let target_vol = 0.1;

        // Scale inversely with volatility
        let vol_multiplier = target_vol / volatility;

        let risk_parity_size = portfolio_value
            * self.config.max_single_position
            * Decimal::try_from(vol_multiplier.min(2.0)).unwrap_or(Decimal::ONE);

        risk_parity_size.min(base_size)
    }

    /// Apply position size constraints.
    fn apply_constraints(
        &self,
        size: Decimal,
        portfolio_value: Decimal,
        current_exposure: Decimal,
    ) -> Decimal {
        let mut final_size = size;

        // Min/max trade size
        final_size = final_size.max(self.config.min_trade_size);
        final_size = final_size.min(self.config.max_trade_size);

        // Max single position constraint
        let max_position = portfolio_value * self.config.max_single_position;
        final_size = final_size.min(max_position);

        // Max portfolio allocation constraint
        let max_total = portfolio_value * self.config.max_portfolio_allocation;
        let remaining_capacity = max_total - current_exposure;
        if remaining_capacity <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        final_size = final_size.min(remaining_capacity);

        // Final sanity check
        if final_size < self.config.min_trade_size {
            return Decimal::ZERO;
        }

        final_size
    }

    /// Calculate aggregate size when copying multiple traders for the same market.
    pub fn aggregate_trader_sizes(
        &self,
        trader_allocations: &[(Decimal, &TraderMetrics)],
        our_portfolio_value: Decimal,
    ) -> Decimal {
        if trader_allocations.is_empty() {
            return Decimal::ZERO;
        }

        // Weight by composite score
        let total_score: f64 = trader_allocations
            .iter()
            .map(|(_, m)| m.composite_score())
            .sum();

        if total_score <= 0.0 {
            return Decimal::ZERO;
        }

        let mut weighted_size = Decimal::ZERO;

        for (base_size, metrics) in trader_allocations {
            let weight = metrics.composite_score() / total_score;
            weighted_size += *base_size * Decimal::try_from(weight).unwrap_or(Decimal::ZERO);
        }

        weighted_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kelly_sizing() {
        let config = TradingConfig::default();
        let sizer = PositionSizer::new(config);

        let mut metrics = TraderMetrics::new("0x123".to_string());
        metrics.win_rate = 0.6;
        metrics.avg_win = dec!(100);
        metrics.avg_loss = dec!(80);
        metrics.max_drawdown = 0.2;

        let size = sizer.calculate_size(
            dec!(100),          // Source trade size
            dec!(10000),        // Source portfolio
            dec!(1000),         // Our portfolio
            Some(&metrics),
            Decimal::ZERO,      // Current exposure
        );

        // Should be reduced by Kelly and our smaller portfolio
        assert!(size > Decimal::ZERO);
        assert!(size < dec!(100)); // Less than source due to Kelly
    }

    #[test]
    fn test_constraints() {
        let config = TradingConfig {
            max_single_position: dec!(0.1),
            max_trade_size: dec!(50),
            ..Default::default()
        };
        let sizer = PositionSizer::new(config);

        let size = sizer.calculate_size(
            dec!(1000),         // Large source trade
            dec!(10000),
            dec!(1000),         // Our portfolio: $1000
            None,
            Decimal::ZERO,
        );

        // Should be capped at max_trade_size or 10% of portfolio
        assert!(size <= dec!(100)); // 10% of $1000
        assert!(size <= dec!(50));  // Max trade size
    }
}
