//! Bot runner: main orchestration loop with full state management.
//!
//! Handles:
//! - Polling for new trades from tracked traders
//! - Validating trades against strategy rules
//! - Executing copy trades via CLOB
//! - Managing positions and evaluating exit signals
//! - Persisting state for crash recovery

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::api::{ClobClient, DataClient, OrderResponse, OrderSide, TradeResponse};
use crate::db::{Database, StoredCopyTrade, StoredPosition};
use crate::models::{Trade, TradeSide};
use crate::trading::{
    CopyEngine, CopyTradeIntent, PortfolioState, Strategy, StrategyConfig, StrategyPosition,
    TradingConfig,
};

/// Bot configuration.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Initial portfolio value in USDC
    pub portfolio_value: Decimal,

    /// Polling interval for new trades (seconds)
    pub poll_interval_secs: u64,

    /// Whether to actually execute trades or just simulate
    pub dry_run: bool,

    /// Trading configuration
    pub trading_config: TradingConfig,

    /// Strategy configuration
    pub strategy_config: StrategyConfig,

    /// Database URL
    pub database_url: String,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            portfolio_value: dec!(1000),
            poll_interval_secs: 30,
            dry_run: true,
            trading_config: TradingConfig::default(),
            strategy_config: StrategyConfig::default(),
            database_url: "sqlite:copybot.db?mode=rwc".to_string(),
        }
    }
}

/// Main bot runner.
pub struct Bot {
    config: BotConfig,
    db: Database,
    data_client: DataClient,
    clob_client: Option<ClobClient>,
    copy_engine: CopyEngine,
    strategy: Strategy,

    // Runtime state
    portfolio_value: Arc<RwLock<Decimal>>,
    cash_available: Arc<RwLock<Decimal>>,
    total_exposure: Arc<RwLock<Decimal>>,
    unrealized_pnl: Arc<RwLock<Decimal>>,
    realized_pnl: Arc<RwLock<Decimal>>,
    peak_equity: Arc<RwLock<Decimal>>,
    last_trade_at: Arc<RwLock<Option<chrono::DateTime<Utc>>>>,
    last_loss_at: Arc<RwLock<Option<chrono::DateTime<Utc>>>>,

    // Shutdown signal
    shutdown: Arc<AtomicBool>,
}

impl Bot {
    /// Create a new bot instance.
    pub async fn new(config: BotConfig) -> Result<Self> {
        let db = Database::new(&config.database_url).await?;
        let data_client = DataClient::new()?;
        let copy_engine = CopyEngine::new(config.trading_config.clone())?;
        let strategy = Strategy::new(config.strategy_config.clone());

        // Initialize CLOB client if not in dry-run mode
        let clob_client = if !config.dry_run {
            match ClobClient::from_env() {
                Ok(client) => {
                    info!(address = ?client.address(), "CLOB client initialized");
                    Some(client)
                }
                Err(e) => {
                    warn!("CLOB client not configured: {}. Running in dry-run mode.", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            config: config.clone(),
            db,
            data_client,
            clob_client,
            copy_engine,
            strategy,
            portfolio_value: Arc::new(RwLock::new(config.portfolio_value)),
            cash_available: Arc::new(RwLock::new(config.portfolio_value)),
            total_exposure: Arc::new(RwLock::new(Decimal::ZERO)),
            unrealized_pnl: Arc::new(RwLock::new(Decimal::ZERO)),
            realized_pnl: Arc::new(RwLock::new(Decimal::ZERO)),
            peak_equity: Arc::new(RwLock::new(config.portfolio_value)),
            last_trade_at: Arc::new(RwLock::new(None)),
            last_loss_at: Arc::new(RwLock::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get shutdown signal for external control.
    pub fn shutdown_signal(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Initialize bot state from database or fresh start.
    pub async fn initialize(&mut self) -> Result<()> {
        info!("Initializing bot...");

        // Initialize or restore bot state
        let portfolio_value = self.config.portfolio_value.to_f64().unwrap_or(1000.0);
        let bot_state = self.db.init_bot_state(portfolio_value).await?;

        // Restore state if resuming
        if bot_state.total_trades > 0 {
            info!(
                total_trades = bot_state.total_trades,
                total_pnl = bot_state.total_pnl,
                "Resuming from previous session"
            );

            *self.realized_pnl.write().await = Decimal::try_from(bot_state.total_pnl)?;
            *self.total_exposure.write().await = Decimal::try_from(bot_state.current_exposure)?;
        }

        // Load tracked traders
        let tracked_addresses = self.db.get_tracked_addresses().await?;
        info!(count = tracked_addresses.len(), "Loading tracked traders");

        for address in tracked_addresses {
            if let Err(e) = self.copy_engine.add_trader(address.clone()).await {
                warn!(address = %address, error = %e, "Failed to load trader");
            }
        }

        // Update copy engine with portfolio value
        self.copy_engine.set_portfolio_value(self.config.portfolio_value).await;

        // Restore positions from database
        let positions = self.db.get_open_positions().await?;
        let exposure: Decimal = positions
            .iter()
            .map(|p| Decimal::try_from(p.size * p.current_price).unwrap_or(Decimal::ZERO))
            .sum();
        *self.total_exposure.write().await = exposure;
        *self.cash_available.write().await = self.config.portfolio_value - exposure;

        info!(
            portfolio = %self.config.portfolio_value,
            exposure = %exposure,
            positions = positions.len(),
            "Bot initialized"
        );

        Ok(())
    }

    /// Main run loop.
    pub async fn run(&mut self) -> Result<()> {
        info!(
            dry_run = self.config.dry_run,
            poll_interval = self.config.poll_interval_secs,
            "Starting bot run loop"
        );

        let mut poll_interval = interval(Duration::from_secs(self.config.poll_interval_secs));

        // Register shutdown handler
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            shutdown.store(true, Ordering::SeqCst);
        });

        while !self.shutdown.load(Ordering::SeqCst) {
            poll_interval.tick().await;

            if let Err(e) = self.tick().await {
                error!(error = %e, "Error in bot tick");
                // Continue running unless it's a critical error
            }
        }

        // Graceful shutdown
        self.shutdown().await?;

        Ok(())
    }

    /// Single iteration of the main loop.
    async fn tick(&mut self) -> Result<()> {
        debug!("Bot tick");

        // 1. Check portfolio risk - halt if necessary
        let portfolio = self.build_portfolio_state().await;
        let (should_halt, halt_reason) = self.strategy.should_halt_trading(&portfolio);
        if should_halt {
            warn!(reason = %halt_reason, "Trading halted due to risk limits");
            return Ok(());
        }

        // 2. Update position prices and check exits
        self.update_positions().await?;
        self.check_exits().await?;

        // 3. Poll for new trades
        let new_intents = self.copy_engine.poll_for_trades().await?;

        // 4. Validate and execute new trades
        for intent in new_intents {
            if let Err(e) = self.process_trade_intent(intent).await {
                warn!(error = %e, "Failed to process trade intent");
            }
        }

        // 5. Process any pending trades from database
        self.process_pending_trades().await?;

        // 6. Record equity point
        self.record_equity().await?;

        // 7. Update bot state
        self.update_bot_state().await?;

        Ok(())
    }

    /// Process a new copy trade intent.
    async fn process_trade_intent(&mut self, intent: CopyTradeIntent) -> Result<()> {
        let trade = &intent.source_trade;

        // Check if we've already seen this trade
        let trade_id = format!(
            "{}-{}-{}",
            trade.trader_address, trade.market_id, trade.timestamp.timestamp()
        );
        if self.db.has_seen_trade(&trade_id).await? {
            debug!(trade_id = %trade_id, "Trade already seen, skipping");
            return Ok(());
        }

        // Get current market price
        let current_price = self.get_current_price(&trade.market_id, &trade.outcome).await?;

        // Validate entry
        let portfolio = self.build_portfolio_state().await;
        let market_positions = self.get_market_positions(&trade.market_id).await?;

        let validation = self.strategy.validate_entry(
            trade.timestamp,
            current_price,
            trade.price,
            intent.calculated_size,
            None, // Would fetch trader metrics here
            &portfolio,
            &market_positions,
        );

        if !validation.allowed {
            info!(
                market = %trade.market_id,
                reason = %validation.reason,
                "Trade rejected by strategy"
            );
            self.db.mark_trade_seen(&trade_id, &trade.trader_address, &trade.market_id).await?;
            return Ok(());
        }

        let size = validation.adjusted_size.unwrap_or(intent.calculated_size);

        // Create copy trade record
        let copy_trade_id = uuid::Uuid::new_v4().to_string();
        self.db.save_copy_trade(
            &copy_trade_id,
            &intent.source_trader,
            &trade_id,
            &trade.market_id,
            "", // market title
            &format!("{:?}", trade.side),
            &trade.outcome,
            trade.amount_usdc.to_f64().unwrap_or(0.0),
            trade.price.to_f64().unwrap_or(0.0),
            size.to_f64().unwrap_or(0.0),
        ).await?;

        // Mark trade as seen
        self.db.mark_trade_seen(&trade_id, &trade.trader_address, &trade.market_id).await?;

        // Execute the trade
        if self.config.dry_run || self.clob_client.is_none() {
            info!(
                market = %trade.market_id,
                side = ?trade.side,
                size = %size,
                price = %current_price,
                "[DRY RUN] Would execute trade"
            );

            // Simulate execution
            self.db.update_copy_trade_status(
                &copy_trade_id,
                "simulated",
                None,
                Some(current_price.to_f64().unwrap_or(0.0)),
                None,
                None,
            ).await?;

            // Update position
            self.update_position_after_trade(
                &trade.market_id,
                &trade.outcome,
                &trade.side,
                size,
                current_price,
                Some(&intent.source_trader),
            ).await?;
        } else {
            // Real execution
            let result = self.execute_trade(
                &trade.market_id,
                &trade.outcome,
                &trade.side,
                size,
            ).await;

            match result {
                Ok(response) => {
                    info!(
                        order_id = ?response.order_id,
                        market = %trade.market_id,
                        "Trade executed"
                    );

                    self.db.update_copy_trade_status(
                        &copy_trade_id,
                        "executed",
                        response.order_id.as_deref(),
                        Some(current_price.to_f64().unwrap_or(0.0)),
                        response.transaction_hash.as_deref(),
                        None,
                    ).await?;

                    // Update position
                    self.update_position_after_trade(
                        &trade.market_id,
                        &trade.outcome,
                        &trade.side,
                        size,
                        current_price,
                        Some(&intent.source_trader),
                    ).await?;
                }
                Err(e) => {
                    error!(error = %e, "Trade execution failed");
                    self.db.update_copy_trade_status(
                        &copy_trade_id,
                        "failed",
                        None,
                        None,
                        None,
                        Some(&e.to_string()),
                    ).await?;
                }
            }
        }

        // Update last trade time
        *self.last_trade_at.write().await = Some(Utc::now());

        Ok(())
    }

    /// Execute a trade via CLOB.
    async fn execute_trade(
        &self,
        market_id: &str,
        outcome: &str,
        side: &TradeSide,
        size: Decimal,
    ) -> Result<OrderResponse> {
        let clob = self.clob_client.as_ref()
            .context("CLOB client not configured")?;

        // Get token ID for this outcome
        // In production, this would come from the market info
        let token_id = format!("{}:{}", market_id, outcome);

        let order_side = match side {
            TradeSide::Buy => OrderSide::Buy,
            TradeSide::Sell => OrderSide::Sell,
        };

        clob.market_order(&token_id, order_side, size).await
    }

    /// Update positions after a trade.
    async fn update_position_after_trade(
        &self,
        market_id: &str,
        outcome: &str,
        side: &TradeSide,
        size: Decimal,
        price: Decimal,
        source_trader: Option<&str>,
    ) -> Result<()> {
        let side_str = match side {
            TradeSide::Buy => "BUY",
            TradeSide::Sell => "SELL",
        };

        self.db.save_position(
            market_id,
            "", // market title
            outcome,
            side_str,
            size.to_f64().unwrap_or(0.0),
            price.to_f64().unwrap_or(0.0),
            source_trader,
        ).await?;

        // Update portfolio state
        let cost = size * price;
        if matches!(side, TradeSide::Buy) {
            *self.cash_available.write().await -= cost;
            *self.total_exposure.write().await += cost;
        }

        Ok(())
    }

    /// Get current price for a market outcome.
    async fn get_current_price(&self, market_id: &str, outcome: &str) -> Result<Decimal> {
        // In production, this would query the order book
        // For now, return a placeholder
        Ok(dec!(0.50))
    }

    /// Get positions for a specific market.
    async fn get_market_positions(&self, market_id: &str) -> Result<Vec<StrategyPosition>> {
        let positions = self.db.get_open_positions().await?;
        Ok(positions
            .iter()
            .filter(|p| p.market_id == market_id)
            .map(|p| self.convert_position(p))
            .collect())
    }

    /// Convert stored position to strategy position.
    fn convert_position(&self, stored: &StoredPosition) -> StrategyPosition {
        StrategyPosition {
            market_id: stored.market_id.clone(),
            outcome: stored.outcome.clone(),
            side: stored.side.clone(),
            entry_price: Decimal::try_from(stored.entry_price).unwrap_or(Decimal::ZERO),
            current_price: Decimal::try_from(stored.current_price).unwrap_or(Decimal::ZERO),
            size: Decimal::try_from(stored.size).unwrap_or(Decimal::ZERO),
            unrealized_pnl: Decimal::try_from(stored.unrealized_pnl).unwrap_or(Decimal::ZERO),
            opened_at: chrono::DateTime::parse_from_rfc3339(&stored.opened_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            source_trader: stored.source_trader.clone(),
        }
    }

    /// Update all position prices.
    async fn update_positions(&mut self) -> Result<()> {
        let positions = self.db.get_open_positions().await?;

        for pos in positions {
            let price = self.get_current_price(&pos.market_id, &pos.outcome).await?;
            self.db.update_position_price(
                &pos.market_id,
                &pos.outcome,
                price.to_f64().unwrap_or(0.0),
            ).await?;
        }

        // Recalculate portfolio state
        let positions = self.db.get_open_positions().await?;
        let mut total_exposure = Decimal::ZERO;
        let mut total_unrealized = Decimal::ZERO;

        for pos in &positions {
            let size = Decimal::try_from(pos.size)?;
            let current = Decimal::try_from(pos.current_price)?;
            let entry = Decimal::try_from(pos.entry_price)?;

            total_exposure += size * current;
            total_unrealized += (current - entry) * size;
        }

        *self.total_exposure.write().await = total_exposure;
        *self.unrealized_pnl.write().await = total_unrealized;
        *self.cash_available.write().await = self.config.portfolio_value - total_exposure + *self.realized_pnl.read().await;

        Ok(())
    }

    /// Check exits for all positions.
    async fn check_exits(&mut self) -> Result<()> {
        let positions = self.db.get_open_positions().await?;
        let portfolio = self.build_portfolio_state().await;

        // Get trader holdings (simplified - would need to fetch from API)
        let trader_holdings: HashMap<String, Vec<String>> = HashMap::new();

        let strategy_positions: Vec<_> = positions.iter().map(|p| self.convert_position(p)).collect();

        let exits = self.strategy.evaluate_exits(&strategy_positions, &portfolio, &trader_holdings);

        for (pos, signal) in exits {
            info!(
                market = %pos.market_id,
                reason = ?signal.reason,
                urgency = ?signal.urgency,
                "Exit signal triggered"
            );

            if self.config.dry_run {
                info!(
                    market = %pos.market_id,
                    size = %pos.size,
                    pnl = %pos.unrealized_pnl,
                    "[DRY RUN] Would exit position"
                );
            } else {
                // Execute exit trade
                let side = if pos.side == "BUY" { TradeSide::Sell } else { TradeSide::Buy };
                if let Err(e) = self.execute_trade(&pos.market_id, &pos.outcome, &side, pos.size).await {
                    error!(error = %e, "Failed to exit position");
                    continue;
                }
            }

            // Update realized P&L
            let realized = pos.unrealized_pnl;
            *self.realized_pnl.write().await += realized;

            if realized < Decimal::ZERO {
                *self.last_loss_at.write().await = Some(Utc::now());
            }

            // Close position in DB
            self.db.close_position(&pos.market_id, &pos.outcome).await?;
        }

        Ok(())
    }

    /// Build current portfolio state.
    async fn build_portfolio_state(&self) -> PortfolioState {
        let total_value = *self.portfolio_value.read().await;
        let cash = *self.cash_available.read().await;
        let exposure = *self.total_exposure.read().await;
        let unrealized = *self.unrealized_pnl.read().await;
        let realized = *self.realized_pnl.read().await;
        let peak = *self.peak_equity.read().await;

        let current_equity = total_value + realized + unrealized;
        let drawdown = if peak > Decimal::ZERO {
            (peak - current_equity).max(Decimal::ZERO) / peak
        } else {
            Decimal::ZERO
        };

        let position_count = self.db.get_open_positions().await
            .map(|p| p.len())
            .unwrap_or(0);

        PortfolioState {
            total_value,
            cash_available: cash,
            total_exposure: exposure,
            unrealized_pnl: unrealized,
            realized_pnl: realized,
            current_drawdown: drawdown,
            position_count,
            last_trade_at: *self.last_trade_at.read().await,
            last_loss_at: *self.last_loss_at.read().await,
        }
    }

    /// Process pending trades from database.
    async fn process_pending_trades(&mut self) -> Result<()> {
        let pending = self.db.get_pending_copy_trades().await?;

        for trade in pending {
            // Retry failed or stuck trades
            debug!(id = %trade.id, "Retrying pending trade");
            // Implementation would retry the trade execution
        }

        Ok(())
    }

    /// Record equity curve point.
    async fn record_equity(&self) -> Result<()> {
        let portfolio = self.build_portfolio_state();
        let portfolio = portfolio.await;

        let equity = portfolio.total_value + portfolio.realized_pnl + portfolio.unrealized_pnl;

        // Update peak
        let mut peak = self.peak_equity.write().await;
        if equity > *peak {
            *peak = equity;
        }

        self.db.record_equity_point(
            equity.to_f64().unwrap_or(0.0),
            portfolio.total_exposure.to_f64().unwrap_or(0.0),
            portfolio.unrealized_pnl.to_f64().unwrap_or(0.0),
            portfolio.realized_pnl.to_f64().unwrap_or(0.0),
        ).await?;

        Ok(())
    }

    /// Update bot state in database.
    async fn update_bot_state(&self) -> Result<()> {
        let exposure = self.total_exposure.read().await.to_f64().unwrap_or(0.0);
        let realized = self.realized_pnl.read().await.to_f64().unwrap_or(0.0);
        let unrealized = self.unrealized_pnl.read().await.to_f64().unwrap_or(0.0);

        let (total, executed, _failed) = self.db.get_copy_trade_stats().await?;

        self.db.update_bot_state(
            exposure,
            realized + unrealized,
            executed,
        ).await?;

        Ok(())
    }

    /// Graceful shutdown.
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down bot...");

        // Mark bot as stopped
        self.db.mark_bot_stopped().await?;

        // Final state save
        self.update_bot_state().await?;

        info!("Bot shutdown complete");
        Ok(())
    }

    /// Add a trader to track.
    pub async fn add_trader(&mut self, address: &str) -> Result<()> {
        self.copy_engine.add_trader(address.to_string()).await?;
        self.db.save_trader(address, "", 1.0).await?;
        Ok(())
    }

    /// Remove a trader from tracking.
    pub async fn remove_trader(&mut self, address: &str) -> Result<()> {
        self.copy_engine.remove_trader(address).await;
        self.db.remove_trader(address).await?;
        Ok(())
    }

    /// Get current stats.
    pub async fn get_stats(&self) -> BotStats {
        let engine_stats = self.copy_engine.get_stats().await;
        let (total_trades, executed, failed) = self.db.get_copy_trade_stats().await.unwrap_or((0, 0, 0));
        let max_dd = self.db.calculate_max_drawdown().await.unwrap_or(0.0);

        BotStats {
            portfolio_value: *self.portfolio_value.read().await,
            cash_available: *self.cash_available.read().await,
            total_exposure: *self.total_exposure.read().await,
            unrealized_pnl: *self.unrealized_pnl.read().await,
            realized_pnl: *self.realized_pnl.read().await,
            max_drawdown: Decimal::try_from(max_dd).unwrap_or(Decimal::ZERO),
            tracked_traders: engine_stats.tracked_traders,
            total_trades,
            executed_trades: executed,
            failed_trades: failed,
            is_running: !self.shutdown.load(Ordering::SeqCst),
            dry_run: self.config.dry_run,
        }
    }
}

/// Bot statistics.
#[derive(Debug, Clone)]
pub struct BotStats {
    pub portfolio_value: Decimal,
    pub cash_available: Decimal,
    pub total_exposure: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub max_drawdown: Decimal,
    pub tracked_traders: usize,
    pub total_trades: i64,
    pub executed_trades: i64,
    pub failed_trades: i64,
    pub is_running: bool,
    pub dry_run: bool,
}

impl std::fmt::Display for BotStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Bot Statistics ===")?;
        writeln!(f, "Portfolio Value: ${:.2}", self.portfolio_value)?;
        writeln!(f, "Cash Available:  ${:.2}", self.cash_available)?;
        writeln!(f, "Total Exposure:  ${:.2}", self.total_exposure)?;
        writeln!(f, "Unrealized P&L:  ${:.2}", self.unrealized_pnl)?;
        writeln!(f, "Realized P&L:    ${:.2}", self.realized_pnl)?;
        writeln!(f, "Max Drawdown:    {:.2}%", self.max_drawdown * dec!(100))?;
        writeln!(f, "Tracked Traders: {}", self.tracked_traders)?;
        writeln!(f, "Total Trades:    {} (Executed: {}, Failed: {})",
            self.total_trades, self.executed_trades, self.failed_trades)?;
        writeln!(f, "Status:          {} {}",
            if self.is_running { "Running" } else { "Stopped" },
            if self.dry_run { "(Dry Run)" } else { "" })?;
        Ok(())
    }
}
