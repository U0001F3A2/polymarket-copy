//! Trade model representing individual trades on Polymarket.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeSide {
    Buy,
    Sell,
}

impl TradeSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeSide::Buy => "BUY",
            TradeSide::Sell => "SELL",
        }
    }
}

/// Individual trade record from Polymarket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    /// Unique trade identifier (typically tx_hash + log_index)
    pub id: String,

    /// Trader's wallet address
    pub trader_address: String,

    /// Market condition ID (0x-prefixed)
    pub market_id: String,

    /// Market title for display
    #[serde(default)]
    pub market_title: String,

    /// Trade direction
    pub side: TradeSide,

    /// Outcome being traded (e.g., "Yes", "No", or specific outcome name)
    pub outcome: String,

    /// Number of outcome tokens traded
    pub size: Decimal,

    /// Price per token in USDC (0.0 to 1.0)
    pub price: Decimal,

    /// Total USDC value of the trade
    pub amount_usdc: Decimal,

    /// When the trade occurred
    pub timestamp: DateTime<Utc>,

    /// On-chain transaction hash
    #[serde(default)]
    pub transaction_hash: String,

    /// Whether this trader was the taker (market order) vs maker (limit order)
    #[serde(default = "default_true")]
    pub is_taker: bool,

    /// Fee paid in USDC
    #[serde(default)]
    pub fee_usdc: Decimal,
}

fn default_true() -> bool {
    true
}

impl Trade {
    /// Calculate the implied probability from the trade price.
    pub fn implied_probability(&self) -> Decimal {
        self.price
    }

    /// Calculate effective cost basis including fees.
    pub fn effective_price(&self) -> Decimal {
        if self.size.is_zero() {
            return Decimal::ZERO;
        }
        (self.amount_usdc + self.fee_usdc) / self.size
    }

    /// Returns true if this is a winning trade (price moved in favorable direction).
    /// Note: This requires knowing the current/final price to determine.
    pub fn is_profitable(&self, current_price: Decimal) -> bool {
        match self.side {
            TradeSide::Buy => current_price > self.price,
            TradeSide::Sell => current_price < self.price,
        }
    }

    /// Calculate P&L if position were closed at given price.
    pub fn calculate_pnl(&self, exit_price: Decimal) -> Decimal {
        let price_diff = exit_price - self.price;
        match self.side {
            TradeSide::Buy => self.size * price_diff,
            TradeSide::Sell => self.size * -price_diff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_pnl_calculation_buy() {
        let trade = Trade {
            id: "test".to_string(),
            trader_address: "0x123".to_string(),
            market_id: "0xabc".to_string(),
            market_title: "Test Market".to_string(),
            side: TradeSide::Buy,
            outcome: "Yes".to_string(),
            size: dec!(100),
            price: dec!(0.50),
            amount_usdc: dec!(50),
            timestamp: Utc::now(),
            transaction_hash: "".to_string(),
            is_taker: true,
            fee_usdc: Decimal::ZERO,
        };

        // Price went up: profitable
        assert_eq!(trade.calculate_pnl(dec!(0.70)), dec!(20)); // 100 * 0.20 = 20
        assert!(trade.is_profitable(dec!(0.70)));

        // Price went down: loss
        assert_eq!(trade.calculate_pnl(dec!(0.30)), dec!(-20)); // 100 * -0.20 = -20
        assert!(!trade.is_profitable(dec!(0.30)));
    }

    #[test]
    fn test_pnl_calculation_sell() {
        let trade = Trade {
            id: "test".to_string(),
            trader_address: "0x123".to_string(),
            market_id: "0xabc".to_string(),
            market_title: "Test Market".to_string(),
            side: TradeSide::Sell,
            outcome: "Yes".to_string(),
            size: dec!(100),
            price: dec!(0.50),
            amount_usdc: dec!(50),
            timestamp: Utc::now(),
            transaction_hash: "".to_string(),
            is_taker: true,
            fee_usdc: Decimal::ZERO,
        };

        // Price went down: profitable for seller
        assert_eq!(trade.calculate_pnl(dec!(0.30)), dec!(20)); // 100 * 0.20 = 20
        assert!(trade.is_profitable(dec!(0.30)));

        // Price went up: loss for seller
        assert_eq!(trade.calculate_pnl(dec!(0.70)), dec!(-20));
        assert!(!trade.is_profitable(dec!(0.70)));
    }
}
