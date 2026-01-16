//! Position model representing a trader's current holdings in a market.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Current position in a prediction market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Trader's wallet address
    pub trader_address: String,

    /// Market condition ID
    pub market_id: String,

    /// Market title for display
    #[serde(default)]
    pub market_title: String,

    /// Outcome token held (e.g., "Yes", "No")
    pub outcome: String,

    /// Number of outcome tokens held
    pub size: Decimal,

    /// Average entry price per token
    pub average_price: Decimal,

    /// Current market price per token
    #[serde(default)]
    pub current_price: Decimal,

    /// Initial cost basis in USDC
    pub initial_value: Decimal,

    /// Current market value in USDC
    #[serde(default)]
    pub current_value: Decimal,

    /// Unrealized P&L in USDC
    #[serde(default)]
    pub unrealized_pnl: Decimal,

    /// Unrealized P&L as percentage
    #[serde(default)]
    pub unrealized_pnl_pct: Decimal,

    /// Last time this position was updated
    #[serde(default = "Utc::now")]
    pub last_updated: DateTime<Utc>,
}

impl Position {
    /// Create a new position from initial trade data.
    pub fn new(
        trader_address: String,
        market_id: String,
        outcome: String,
        size: Decimal,
        price: Decimal,
    ) -> Self {
        let initial_value = size * price;
        Self {
            trader_address,
            market_id,
            market_title: String::new(),
            outcome,
            size,
            average_price: price,
            current_price: price,
            initial_value,
            current_value: initial_value,
            unrealized_pnl: Decimal::ZERO,
            unrealized_pnl_pct: Decimal::ZERO,
            last_updated: Utc::now(),
        }
    }

    /// Update position P&L based on current market price.
    pub fn update_price(&mut self, current_price: Decimal) {
        self.current_price = current_price;
        self.current_value = self.size * current_price;
        self.unrealized_pnl = self.current_value - self.initial_value;

        if !self.initial_value.is_zero() {
            self.unrealized_pnl_pct = self.unrealized_pnl / self.initial_value;
        }

        self.last_updated = Utc::now();
    }

    /// Add to position (averaging in).
    pub fn add(&mut self, size: Decimal, price: Decimal) {
        let new_cost = size * price;
        let total_cost = self.initial_value + new_cost;
        let new_size = self.size + size;

        if !new_size.is_zero() {
            self.average_price = total_cost / new_size;
        }

        self.size = new_size;
        self.initial_value = total_cost;
        self.update_price(self.current_price);
    }

    /// Reduce position size.
    pub fn reduce(&mut self, size: Decimal) -> Decimal {
        let reduce_size = size.min(self.size);
        let realized_pnl = reduce_size * (self.current_price - self.average_price);

        self.size -= reduce_size;
        self.initial_value = self.size * self.average_price;
        self.update_price(self.current_price);

        realized_pnl
    }

    /// Check if this position is closed (size is zero or negligible).
    pub fn is_closed(&self) -> bool {
        self.size < Decimal::new(1, 6) // Less than 0.000001
    }

    /// Calculate the dollar risk (potential loss if price goes to 0).
    pub fn dollar_at_risk(&self) -> Decimal {
        self.current_value
    }

    /// Calculate the potential upside if the outcome resolves to 1.0.
    pub fn potential_profit(&self) -> Decimal {
        self.size - self.initial_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_position_pnl() {
        let mut pos = Position::new(
            "0x123".to_string(),
            "0xmarket".to_string(),
            "Yes".to_string(),
            dec!(100),
            dec!(0.50),
        );

        assert_eq!(pos.initial_value, dec!(50));
        assert_eq!(pos.unrealized_pnl, dec!(0));

        // Price goes up
        pos.update_price(dec!(0.70));
        assert_eq!(pos.current_value, dec!(70));
        assert_eq!(pos.unrealized_pnl, dec!(20));
    }

    #[test]
    fn test_position_averaging() {
        let mut pos = Position::new(
            "0x123".to_string(),
            "0xmarket".to_string(),
            "Yes".to_string(),
            dec!(100),
            dec!(0.50),
        );

        // Add more at different price
        pos.add(dec!(100), dec!(0.60));

        assert_eq!(pos.size, dec!(200));
        // Average: (50 + 60) / 200 = 0.55
        assert_eq!(pos.average_price, dec!(0.55));
        assert_eq!(pos.initial_value, dec!(110));
    }
}
