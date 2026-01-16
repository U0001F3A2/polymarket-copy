//! Copy-trading engine: monitors traders and executes copy trades.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::api::DataClient;
use crate::metrics::MetricsCalculator;
use crate::models::{Trade, Trader, TraderMetrics};

use super::{PositionSizer, TradingConfig};

/// Represents a pending copy trade to be executed.
#[derive(Debug, Clone)]
pub struct CopyTradeIntent {
    pub source_trader: String,
    pub source_trade: Trade,
    pub calculated_size: Decimal,
    pub created_at: DateTime<Utc>,
}

/// Copy-trading engine state.
pub struct CopyEngine {
    config: TradingConfig,
    data_client: DataClient,
    position_sizer: PositionSizer,

    // Tracked traders and their metrics
    tracked_traders: Arc<RwLock<HashMap<String, Trader>>>,

    // Last seen trade ID per trader (to detect new trades)
    last_seen_trades: Arc<RwLock<HashMap<String, String>>>,

    // Our portfolio state
    portfolio_value: Arc<RwLock<Decimal>>,
    current_exposure: Arc<RwLock<Decimal>>,

    // Pending trades to execute
    pending_trades: Arc<RwLock<Vec<CopyTradeIntent>>>,
}

impl CopyEngine {
    /// Create a new copy engine.
    pub fn new(config: TradingConfig) -> Result<Self> {
        let data_client = DataClient::new()?;
        let position_sizer = PositionSizer::new(config.clone());

        Ok(Self {
            config,
            data_client,
            position_sizer,
            tracked_traders: Arc::new(RwLock::new(HashMap::new())),
            last_seen_trades: Arc::new(RwLock::new(HashMap::new())),
            portfolio_value: Arc::new(RwLock::new(Decimal::ZERO)),
            current_exposure: Arc::new(RwLock::new(Decimal::ZERO)),
            pending_trades: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Set our portfolio value.
    pub async fn set_portfolio_value(&self, value: Decimal) {
        *self.portfolio_value.write().await = value;
    }

    /// Add a trader to track.
    pub async fn add_trader(&self, address: String) -> Result<()> {
        let mut trader = Trader::new(address.clone());
        trader.start_tracking();

        // Fetch initial data
        let positions = self.data_client.get_positions(&address, Some(100)).await?;
        let trades = self.data_client.get_trades(&address, Some(200), None).await?;

        trader.positions = positions;

        // Calculate metrics (simplified - would need resolved P&Ls for accuracy)
        let pnls: Vec<Decimal> = trades.iter().map(|_| Decimal::ZERO).collect(); // Placeholder
        let metrics = MetricsCalculator::calculate(&address, &trades, &pnls);
        trader.metrics = Some(metrics);

        // Store last trade ID
        if let Some(last_trade) = trades.first() {
            let mut last_seen = self.last_seen_trades.write().await;
            last_seen.insert(address.clone(), last_trade.id.clone());
        }

        // Check if trader meets requirements
        if let Some(ref m) = trader.metrics {
            if !m.is_quality_trader() {
                warn!(
                    address = %address,
                    win_rate = %m.win_rate,
                    sharpe = %m.sharpe_ratio,
                    "Trader does not meet quality requirements"
                );
            }
        }

        let mut traders = self.tracked_traders.write().await;
        traders.insert(address.clone(), trader);

        info!(address = %address, "Added trader to tracking");
        Ok(())
    }

    /// Remove a trader from tracking.
    pub async fn remove_trader(&self, address: &str) {
        let mut traders = self.tracked_traders.write().await;
        traders.remove(address);

        let mut last_seen = self.last_seen_trades.write().await;
        last_seen.remove(address);

        info!(address = %address, "Removed trader from tracking");
    }

    /// Get all tracked traders.
    pub async fn get_tracked_traders(&self) -> Vec<Trader> {
        let traders = self.tracked_traders.read().await;
        traders.values().cloned().collect()
    }

    /// Poll for new trades from tracked traders.
    pub async fn poll_for_trades(&self) -> Result<Vec<CopyTradeIntent>> {
        let traders = self.tracked_traders.read().await;
        let mut last_seen = self.last_seen_trades.write().await;

        let mut new_intents = Vec::new();

        for (address, trader) in traders.iter() {
            let trades = self
                .data_client
                .get_trades(address, Some(10), None)
                .await?;

            if trades.is_empty() {
                continue;
            }

            let last_seen_id = last_seen.get(address).cloned();

            // Find new trades
            let new_trades: Vec<_> = trades
                .into_iter()
                .take_while(|t| Some(&t.id) != last_seen_id.as_ref())
                .collect();

            if !new_trades.is_empty() {
                // Update last seen
                if let Some(newest) = new_trades.first() {
                    last_seen.insert(address.clone(), newest.id.clone());
                }

                // Calculate copy trade sizes
                let portfolio = *self.portfolio_value.read().await;
                let exposure = *self.current_exposure.read().await;
                let source_value = trader.total_position_value();

                for trade in new_trades {
                    let size = self.position_sizer.calculate_size(
                        trade.amount_usdc,
                        source_value,
                        portfolio,
                        trader.metrics.as_ref(),
                        exposure,
                    );

                    if size > Decimal::ZERO {
                        let intent = CopyTradeIntent {
                            source_trader: address.clone(),
                            source_trade: trade,
                            calculated_size: size,
                            created_at: Utc::now(),
                        };

                        info!(
                            trader = %address,
                            market = %intent.source_trade.market_id,
                            side = ?intent.source_trade.side,
                            size = %intent.calculated_size,
                            "New copy trade intent"
                        );

                        new_intents.push(intent);
                    }
                }
            }
        }

        // Store pending trades
        let mut pending = self.pending_trades.write().await;
        pending.extend(new_intents.clone());

        Ok(new_intents)
    }

    /// Get pending trades.
    pub async fn get_pending_trades(&self) -> Vec<CopyTradeIntent> {
        let pending = self.pending_trades.read().await;
        pending.clone()
    }

    /// Clear pending trades (after execution).
    pub async fn clear_pending_trades(&self) {
        let mut pending = self.pending_trades.write().await;
        pending.clear();
    }

    /// Refresh metrics for all tracked traders.
    pub async fn refresh_trader_metrics(&self) -> Result<()> {
        let mut traders = self.tracked_traders.write().await;

        for (address, trader) in traders.iter_mut() {
            debug!(address = %address, "Refreshing trader metrics");

            let positions = self.data_client.get_positions(address, Some(100)).await?;
            let trades = self.data_client.get_trades(address, Some(500), None).await?;

            trader.positions = positions;

            // Recalculate metrics
            let pnls: Vec<Decimal> = vec![]; // Would need resolved trade data
            let metrics = MetricsCalculator::calculate(address, &trades, &pnls);
            trader.metrics = Some(metrics);
        }

        Ok(())
    }

    /// Discover and optionally add top traders.
    pub async fn discover_traders(&self, min_pnl: f64, limit: usize) -> Result<Vec<Trader>> {
        let traders = self
            .data_client
            .discover_top_traders(min_pnl, "MONTH", limit)
            .await?;

        info!(count = traders.len(), "Discovered traders from leaderboard");
        Ok(traders)
    }

    /// Get aggregated statistics about tracked traders.
    pub async fn get_stats(&self) -> EngineStats {
        let traders = self.tracked_traders.read().await;
        let pending = self.pending_trades.read().await;
        let portfolio = *self.portfolio_value.read().await;
        let exposure = *self.current_exposure.read().await;

        let avg_win_rate = traders
            .values()
            .filter_map(|t| t.metrics.as_ref())
            .map(|m| m.win_rate)
            .sum::<f64>()
            / traders.len().max(1) as f64;

        let avg_sharpe = traders
            .values()
            .filter_map(|t| t.metrics.as_ref())
            .map(|m| m.sharpe_ratio)
            .sum::<f64>()
            / traders.len().max(1) as f64;

        EngineStats {
            tracked_traders: traders.len(),
            pending_trades: pending.len(),
            portfolio_value: portfolio,
            current_exposure: exposure,
            avg_trader_win_rate: avg_win_rate,
            avg_trader_sharpe: avg_sharpe,
        }
    }

    /// Get recent trades for a trader.
    pub async fn get_trader_trades(
        &self,
        address: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Vec<Trade>> {
        self.data_client.get_trades(address, limit, cursor).await
    }
}

/// Engine statistics.
#[derive(Debug, Clone)]
pub struct EngineStats {
    pub tracked_traders: usize,
    pub pending_trades: usize,
    pub portfolio_value: Decimal,
    pub current_exposure: Decimal,
    pub avg_trader_win_rate: f64,
    pub avg_trader_sharpe: f64,
}
