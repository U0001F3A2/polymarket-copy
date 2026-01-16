//! Backtesting engine for validating copy-trading strategies against historical data.
//!
//! Features:
//! - Replay historical trades from tracked traders
//! - Apply strategy rules and position sizing
//! - Track simulated P&L and positions
//! - Calculate performance statistics

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use tracing::{debug, info, warn};

use crate::api::DataClient;
use crate::models::{Trade, TradeSide};
use crate::trading::{PositionSizer, PortfolioState, Strategy, StrategyConfig, StrategyPosition, TradingConfig};

/// Backtesting configuration.
#[derive(Debug, Clone)]
pub struct BacktestConfig {
    /// Starting portfolio value
    pub initial_capital: Decimal,

    /// Trading configuration
    pub trading_config: TradingConfig,

    /// Strategy configuration
    pub strategy_config: StrategyConfig,

    /// Simulated slippage (0.0 to 1.0)
    pub slippage: Decimal,

    /// Trading fee rate (0.0 to 1.0)
    pub fee_rate: Decimal,

    /// Number of historical trades to fetch per trader
    pub lookback_trades: u32,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_capital: dec!(10000),
            trading_config: TradingConfig::default(),
            strategy_config: StrategyConfig::default(),
            slippage: dec!(0.005),  // 0.5% slippage
            fee_rate: dec!(0.001),  // 0.1% fee
            lookback_trades: 500,
        }
    }
}

/// A simulated position during backtesting.
#[derive(Debug, Clone)]
pub struct SimulatedPosition {
    pub market_id: String,
    pub outcome: String,
    pub side: TradeSide,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub entry_time: DateTime<Utc>,
    pub source_trader: String,
}

impl SimulatedPosition {
    /// Calculate P&L at a given price.
    pub fn pnl_at(&self, current_price: Decimal) -> Decimal {
        match self.side {
            TradeSide::Buy => (current_price - self.entry_price) * self.size,
            TradeSide::Sell => (self.entry_price - current_price) * self.size,
        }
    }

    /// Calculate return percentage.
    pub fn return_pct(&self, current_price: Decimal) -> Decimal {
        if self.entry_price.is_zero() {
            return Decimal::ZERO;
        }
        match self.side {
            TradeSide::Buy => (current_price - self.entry_price) / self.entry_price,
            TradeSide::Sell => (self.entry_price - current_price) / self.entry_price,
        }
    }
}

/// A completed trade in the backtest.
#[derive(Debug, Clone)]
pub struct BacktestTrade {
    pub market_id: String,
    pub outcome: String,
    pub side: TradeSide,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub entry_time: DateTime<Utc>,
    pub exit_time: DateTime<Utc>,
    pub pnl: Decimal,
    pub return_pct: Decimal,
    pub source_trader: String,
    pub exit_reason: String,
}

/// Backtest results summary.
#[derive(Debug, Clone)]
pub struct BacktestResults {
    /// Initial capital
    pub initial_capital: Decimal,

    /// Final portfolio value
    pub final_capital: Decimal,

    /// Total return percentage
    pub total_return_pct: Decimal,

    /// Total number of trades
    pub total_trades: usize,

    /// Winning trades
    pub winning_trades: usize,

    /// Losing trades
    pub losing_trades: usize,

    /// Win rate
    pub win_rate: f64,

    /// Average win
    pub avg_win: Decimal,

    /// Average loss
    pub avg_loss: Decimal,

    /// Profit factor
    pub profit_factor: f64,

    /// Maximum drawdown percentage
    pub max_drawdown_pct: f64,

    /// Sharpe ratio (annualized)
    pub sharpe_ratio: f64,

    /// Sortino ratio
    pub sortino_ratio: f64,

    /// Average holding period in hours
    pub avg_holding_hours: f64,

    /// Total fees paid
    pub total_fees: Decimal,

    /// All completed trades
    pub trades: Vec<BacktestTrade>,

    /// Equity curve (timestamp, equity)
    pub equity_curve: Vec<(DateTime<Utc>, Decimal)>,

    /// Trades skipped due to strategy rules
    pub skipped_trades: usize,

    /// Start time of backtest period
    pub start_time: DateTime<Utc>,

    /// End time of backtest period
    pub end_time: DateTime<Utc>,
}

impl std::fmt::Display for BacktestResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "\n{:=^60}", " BACKTEST RESULTS ")?;
        writeln!(f)?;
        writeln!(f, "Period: {} to {}",
            self.start_time.format("%Y-%m-%d"),
            self.end_time.format("%Y-%m-%d"))?;
        writeln!(f)?;
        writeln!(f, "--- Capital ---")?;
        writeln!(f, "Initial:     ${:.2}", self.initial_capital)?;
        writeln!(f, "Final:       ${:.2}", self.final_capital)?;
        writeln!(f, "Return:      {:.2}%", self.total_return_pct * dec!(100))?;
        writeln!(f, "Fees Paid:   ${:.2}", self.total_fees)?;
        writeln!(f)?;
        writeln!(f, "--- Trades ---")?;
        writeln!(f, "Total:       {} ({} skipped)", self.total_trades, self.skipped_trades)?;
        writeln!(f, "Winners:     {} ({:.1}%)", self.winning_trades, self.win_rate * 100.0)?;
        writeln!(f, "Losers:      {}", self.losing_trades)?;
        writeln!(f, "Avg Win:     ${:.2}", self.avg_win)?;
        writeln!(f, "Avg Loss:    ${:.2}", self.avg_loss)?;
        writeln!(f, "Profit Factor: {:.2}", self.profit_factor)?;
        writeln!(f)?;
        writeln!(f, "--- Risk Metrics ---")?;
        writeln!(f, "Max Drawdown: {:.2}%", self.max_drawdown_pct * 100.0)?;
        writeln!(f, "Sharpe Ratio: {:.2}", self.sharpe_ratio)?;
        writeln!(f, "Sortino Ratio: {:.2}", self.sortino_ratio)?;
        writeln!(f)?;
        writeln!(f, "--- Timing ---")?;
        writeln!(f, "Avg Hold:    {:.1} hours", self.avg_holding_hours)?;
        writeln!(f, "{:=^60}", "")?;
        Ok(())
    }
}

/// Backtesting engine.
pub struct Backtester {
    config: BacktestConfig,
    data_client: DataClient,
    strategy: Strategy,
    position_sizer: PositionSizer,
}

impl Backtester {
    /// Create a new backtester.
    pub fn new(config: BacktestConfig) -> Result<Self> {
        let data_client = DataClient::new()?;
        let strategy = Strategy::new(config.strategy_config.clone());
        let position_sizer = PositionSizer::new(config.trading_config.clone());

        Ok(Self {
            config,
            data_client,
            strategy,
            position_sizer,
        })
    }

    /// Run a backtest for a single trader.
    pub async fn run_single_trader(&self, trader_address: &str) -> Result<BacktestResults> {
        info!(trader = %trader_address, "Starting backtest");

        // Fetch historical trades
        let trades = self.data_client
            .get_trades(trader_address, Some(self.config.lookback_trades), None)
            .await
            .context("Failed to fetch historical trades")?;

        if trades.is_empty() {
            return Err(anyhow::anyhow!("No historical trades found for trader"));
        }

        info!(count = trades.len(), "Fetched historical trades");

        // Sort trades by timestamp (oldest first)
        let mut sorted_trades = trades;
        sorted_trades.sort_by_key(|t| t.timestamp);

        self.run_simulation(trader_address, &sorted_trades).await
    }

    /// Run a backtest for multiple traders.
    pub async fn run_multiple_traders(&self, trader_addresses: &[String]) -> Result<BacktestResults> {
        info!(count = trader_addresses.len(), "Starting multi-trader backtest");

        // Fetch trades for all traders
        let mut all_trades: Vec<(String, Trade)> = Vec::new();

        for address in trader_addresses {
            match self.data_client
                .get_trades(address, Some(self.config.lookback_trades), None)
                .await
            {
                Ok(trades) => {
                    info!(trader = %address, count = trades.len(), "Fetched trades");
                    for trade in trades {
                        all_trades.push((address.clone(), trade));
                    }
                }
                Err(e) => {
                    warn!(trader = %address, error = %e, "Failed to fetch trades");
                }
            }
        }

        if all_trades.is_empty() {
            return Err(anyhow::anyhow!("No historical trades found"));
        }

        // Sort by timestamp
        all_trades.sort_by_key(|(_, t)| t.timestamp);

        // Convert to format expected by simulation
        let trades_only: Vec<Trade> = all_trades.iter().map(|(_, t)| t.clone()).collect();
        let trader_map: HashMap<usize, String> = all_trades.iter()
            .enumerate()
            .map(|(i, (addr, _))| (i, addr.clone()))
            .collect();

        self.run_simulation_multi(&trades_only, &trader_map).await
    }

    /// Run the simulation on sorted trades.
    async fn run_simulation(&self, trader_address: &str, trades: &[Trade]) -> Result<BacktestResults> {
        let trader_map: HashMap<usize, String> = trades.iter()
            .enumerate()
            .map(|(i, _)| (i, trader_address.to_string()))
            .collect();

        self.run_simulation_multi(trades, &trader_map).await
    }

    /// Run simulation with multiple traders mapped.
    async fn run_simulation_multi(
        &self,
        trades: &[Trade],
        trader_map: &HashMap<usize, String>,
    ) -> Result<BacktestResults> {
        let mut capital = self.config.initial_capital;
        let mut positions: HashMap<String, SimulatedPosition> = HashMap::new();
        let mut completed_trades: Vec<BacktestTrade> = Vec::new();
        let mut equity_curve: Vec<(DateTime<Utc>, Decimal)> = Vec::new();
        let mut total_fees = Decimal::ZERO;
        let mut skipped = 0;
        let mut peak_equity = capital;
        let mut max_drawdown = 0.0f64;
        let mut last_trade_time: Option<DateTime<Utc>> = None;
        let mut last_loss_time: Option<DateTime<Utc>> = None;

        let start_time = trades.first().map(|t| t.timestamp).unwrap_or_else(Utc::now);
        let end_time = trades.last().map(|t| t.timestamp).unwrap_or_else(Utc::now);

        // Record initial equity
        equity_curve.push((start_time, capital));

        for (idx, trade) in trades.iter().enumerate() {
            let trader = trader_map.get(&idx).cloned().unwrap_or_default();
            let position_key = format!("{}:{}", trade.market_id, trade.outcome);

            // Check if this is an exit trade (we have opposite position)
            if let Some(existing) = positions.get(&position_key) {
                if existing.side != trade.side {
                    // This is an exit - close the position
                    let exit_price = self.apply_slippage(trade.price, trade.side);
                    let pnl = existing.pnl_at(exit_price);
                    let return_pct = existing.return_pct(exit_price);

                    // Apply fees
                    let fee = exit_price * existing.size * self.config.fee_rate;
                    total_fees += fee;
                    let net_pnl = pnl - fee;

                    capital += existing.size * existing.entry_price + net_pnl;

                    completed_trades.push(BacktestTrade {
                        market_id: existing.market_id.clone(),
                        outcome: existing.outcome.clone(),
                        side: existing.side.clone(),
                        size: existing.size,
                        entry_price: existing.entry_price,
                        exit_price,
                        entry_time: existing.entry_time,
                        exit_time: trade.timestamp,
                        pnl: net_pnl,
                        return_pct,
                        source_trader: existing.source_trader.clone(),
                        exit_reason: "Trader Exit".to_string(),
                    });

                    if net_pnl < Decimal::ZERO {
                        last_loss_time = Some(trade.timestamp);
                    }

                    positions.remove(&position_key);
                    last_trade_time = Some(trade.timestamp);

                    debug!(
                        market = %trade.market_id,
                        pnl = %net_pnl,
                        "Closed position"
                    );

                    continue;
                }
            }

            // Build portfolio state for validation
            let exposure: Decimal = positions.values()
                .map(|p| p.size * p.entry_price)
                .sum();

            let unrealized: Decimal = positions.values()
                .map(|p| p.pnl_at(trade.price))
                .sum();

            let current_equity = capital + unrealized;
            let drawdown = if peak_equity > Decimal::ZERO {
                ((peak_equity - current_equity) / peak_equity).to_f64().unwrap_or(0.0)
            } else {
                0.0
            };

            let portfolio = PortfolioState {
                total_value: self.config.initial_capital,
                cash_available: capital,
                total_exposure: exposure,
                unrealized_pnl: unrealized,
                realized_pnl: current_equity - self.config.initial_capital - unrealized,
                current_drawdown: Decimal::try_from(drawdown).unwrap_or(Decimal::ZERO),
                position_count: positions.len(),
                last_trade_at: last_trade_time,
                last_loss_at: last_loss_time,
            };

            // Get market positions for this market
            let market_positions: Vec<StrategyPosition> = positions.values()
                .filter(|p| p.market_id == trade.market_id)
                .map(|p| StrategyPosition {
                    market_id: p.market_id.clone(),
                    outcome: p.outcome.clone(),
                    side: format!("{:?}", p.side),
                    entry_price: p.entry_price,
                    current_price: trade.price,
                    size: p.size,
                    unrealized_pnl: p.pnl_at(trade.price),
                    opened_at: p.entry_time,
                    source_trader: Some(p.source_trader.clone()),
                })
                .collect();

            // Calculate position size
            let source_portfolio = dec!(10000); // Assumed trader portfolio
            let base_size = self.position_sizer.calculate_size(
                trade.amount_usdc,
                source_portfolio,
                self.config.initial_capital,
                None,
                exposure,
            );

            // Validate entry
            let validation = self.strategy.validate_entry(
                trade.timestamp,
                trade.price,
                trade.price, // No slippage check in backtest source
                base_size,
                None,
                &portfolio,
                &market_positions,
            );

            if !validation.allowed {
                debug!(
                    market = %trade.market_id,
                    reason = %validation.reason,
                    "Trade skipped"
                );
                skipped += 1;
                continue;
            }

            let size = validation.adjusted_size.unwrap_or(base_size);
            if size <= Decimal::ZERO {
                skipped += 1;
                continue;
            }

            // Apply slippage to entry
            let entry_price = self.apply_slippage(trade.price, trade.side);

            // Apply entry fee
            let entry_fee = entry_price * size * self.config.fee_rate;
            total_fees += entry_fee;

            // Deduct capital
            let cost = entry_price * size + entry_fee;
            if cost > capital {
                debug!(
                    market = %trade.market_id,
                    cost = %cost,
                    capital = %capital,
                    "Insufficient capital"
                );
                skipped += 1;
                continue;
            }

            capital -= cost;

            // Open position
            positions.insert(position_key.clone(), SimulatedPosition {
                market_id: trade.market_id.clone(),
                outcome: trade.outcome.clone(),
                side: trade.side.clone(),
                size,
                entry_price,
                entry_time: trade.timestamp,
                source_trader: trader.clone(),
            });

            last_trade_time = Some(trade.timestamp);

            debug!(
                market = %trade.market_id,
                side = ?trade.side,
                size = %size,
                price = %entry_price,
                "Opened position"
            );

            // Update equity tracking
            let current_equity = capital + positions.values()
                .map(|p| p.size * p.entry_price + p.pnl_at(trade.price))
                .sum::<Decimal>();

            if current_equity > peak_equity {
                peak_equity = current_equity;
            }

            let dd = ((peak_equity - current_equity) / peak_equity)
                .to_f64()
                .unwrap_or(0.0);
            if dd > max_drawdown {
                max_drawdown = dd;
            }

            equity_curve.push((trade.timestamp, current_equity));
        }

        // Close any remaining positions at last known price
        for (_, pos) in positions.drain() {
            // Use entry price as exit (conservative)
            let exit_price = pos.entry_price;
            let pnl = Decimal::ZERO; // Assume flat

            capital += pos.size * pos.entry_price;

            completed_trades.push(BacktestTrade {
                market_id: pos.market_id,
                outcome: pos.outcome,
                side: pos.side,
                size: pos.size,
                entry_price: pos.entry_price,
                exit_price,
                entry_time: pos.entry_time,
                exit_time: end_time,
                pnl,
                return_pct: Decimal::ZERO,
                source_trader: pos.source_trader,
                exit_reason: "End of Backtest".to_string(),
            });
        }

        // Calculate statistics
        let final_equity = capital;
        let total_return = (final_equity - self.config.initial_capital) / self.config.initial_capital;

        let winners: Vec<_> = completed_trades.iter().filter(|t| t.pnl > Decimal::ZERO).collect();
        let losers: Vec<_> = completed_trades.iter().filter(|t| t.pnl < Decimal::ZERO).collect();

        let win_rate = if !completed_trades.is_empty() {
            winners.len() as f64 / completed_trades.len() as f64
        } else {
            0.0
        };

        let avg_win = if !winners.is_empty() {
            winners.iter().map(|t| t.pnl).sum::<Decimal>() / Decimal::from(winners.len())
        } else {
            Decimal::ZERO
        };

        let avg_loss = if !losers.is_empty() {
            losers.iter().map(|t| t.pnl.abs()).sum::<Decimal>() / Decimal::from(losers.len())
        } else {
            Decimal::ZERO
        };

        let gross_profit: Decimal = winners.iter().map(|t| t.pnl).sum();
        let gross_loss: Decimal = losers.iter().map(|t| t.pnl.abs()).sum();
        let profit_factor = if gross_loss > Decimal::ZERO {
            gross_profit.to_f64().unwrap_or(0.0) / gross_loss.to_f64().unwrap_or(1.0)
        } else {
            f64::INFINITY
        };

        let avg_holding: f64 = if !completed_trades.is_empty() {
            completed_trades.iter()
                .map(|t| (t.exit_time - t.entry_time).num_hours() as f64)
                .sum::<f64>() / completed_trades.len() as f64
        } else {
            0.0
        };

        // Calculate Sharpe/Sortino from equity curve
        let (sharpe, sortino) = self.calculate_risk_ratios(&equity_curve);

        equity_curve.push((end_time, final_equity));

        Ok(BacktestResults {
            initial_capital: self.config.initial_capital,
            final_capital: final_equity,
            total_return_pct: total_return,
            total_trades: completed_trades.len(),
            winning_trades: winners.len(),
            losing_trades: losers.len(),
            win_rate,
            avg_win,
            avg_loss,
            profit_factor,
            max_drawdown_pct: max_drawdown,
            sharpe_ratio: sharpe,
            sortino_ratio: sortino,
            avg_holding_hours: avg_holding,
            total_fees,
            trades: completed_trades,
            equity_curve,
            skipped_trades: skipped,
            start_time,
            end_time,
        })
    }

    /// Apply slippage to a price.
    fn apply_slippage(&self, price: Decimal, side: TradeSide) -> Decimal {
        match side {
            TradeSide::Buy => price * (Decimal::ONE + self.config.slippage),
            TradeSide::Sell => price * (Decimal::ONE - self.config.slippage),
        }
    }

    /// Calculate Sharpe and Sortino ratios from equity curve.
    fn calculate_risk_ratios(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> (f64, f64) {
        if equity_curve.len() < 2 {
            return (0.0, 0.0);
        }

        // Calculate returns
        let returns: Vec<f64> = equity_curve.windows(2)
            .filter_map(|w| {
                let prev = w[0].1.to_f64()?;
                let curr = w[1].1.to_f64()?;
                if prev > 0.0 {
                    Some((curr - prev) / prev)
                } else {
                    None
                }
            })
            .collect();

        if returns.is_empty() {
            return (0.0, 0.0);
        }

        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;

        let variance: f64 = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        let sharpe = if std_dev > 0.0 {
            (mean / std_dev) * (252.0_f64).sqrt() // Annualized
        } else {
            0.0
        };

        // Sortino (downside deviation)
        let negative_returns: Vec<f64> = returns.iter()
            .filter(|&&r| r < 0.0)
            .copied()
            .collect();

        let sortino = if !negative_returns.is_empty() {
            let downside_variance: f64 = negative_returns.iter()
                .map(|r| r.powi(2))
                .sum::<f64>() / negative_returns.len() as f64;
            let downside_dev = downside_variance.sqrt();

            if downside_dev > 0.0 {
                (mean / downside_dev) * (252.0_f64).sqrt()
            } else {
                0.0
            }
        } else {
            0.0
        };

        (sharpe, sortino)
    }
}

// ============== Paper Trading Simulator ==============

/// Paper trading configuration.
#[derive(Debug, Clone)]
pub struct PaperConfig {
    /// Starting capital
    pub initial_capital: Decimal,

    /// Trading configuration
    pub trading_config: TradingConfig,

    /// Strategy configuration
    pub strategy_config: StrategyConfig,

    /// Simulated slippage
    pub slippage: Decimal,

    /// Simulated fee rate
    pub fee_rate: Decimal,
}

impl Default for PaperConfig {
    fn default() -> Self {
        Self {
            initial_capital: dec!(10000),
            trading_config: TradingConfig::default(),
            strategy_config: StrategyConfig::default(),
            slippage: dec!(0.003),
            fee_rate: dec!(0.001),
        }
    }
}

/// Paper trading state.
pub struct PaperTrader {
    pub config: PaperConfig,
    pub capital: Decimal,
    pub positions: HashMap<String, SimulatedPosition>,
    pub completed_trades: Vec<BacktestTrade>,
    pub equity_curve: Vec<(DateTime<Utc>, Decimal)>,
    pub total_fees: Decimal,
    pub peak_equity: Decimal,
    pub started_at: DateTime<Utc>,
    strategy: Strategy,
    position_sizer: PositionSizer,
}

impl PaperTrader {
    /// Create a new paper trader.
    pub fn new(config: PaperConfig) -> Self {
        let strategy = Strategy::new(config.strategy_config.clone());
        let position_sizer = PositionSizer::new(config.trading_config.clone());

        Self {
            capital: config.initial_capital,
            positions: HashMap::new(),
            completed_trades: Vec::new(),
            equity_curve: vec![(Utc::now(), config.initial_capital)],
            total_fees: Decimal::ZERO,
            peak_equity: config.initial_capital,
            started_at: Utc::now(),
            strategy,
            position_sizer,
            config,
        }
    }

    /// Get current equity (capital + unrealized P&L).
    pub fn current_equity(&self, prices: &HashMap<String, Decimal>) -> Decimal {
        let unrealized: Decimal = self.positions.iter()
            .map(|(key, pos)| {
                let price = prices.get(key).copied().unwrap_or(pos.entry_price);
                pos.pnl_at(price)
            })
            .sum();

        self.capital + self.positions.values()
            .map(|p| p.size * p.entry_price)
            .sum::<Decimal>() + unrealized
    }

    /// Process a new trade from a tracked trader.
    pub fn process_trade(
        &mut self,
        trade: &Trade,
        source_trader: &str,
        current_price: Decimal,
    ) -> Result<Option<String>> {
        let position_key = format!("{}:{}", trade.market_id, trade.outcome);

        // Check if this is an exit
        if let Some(existing) = self.positions.get(&position_key) {
            if existing.side != trade.side {
                return self.close_position(&position_key, current_price, "Trader Exit");
            }
        }

        // Build portfolio state
        let exposure: Decimal = self.positions.values()
            .map(|p| p.size * p.entry_price)
            .sum();

        let portfolio = PortfolioState {
            total_value: self.config.initial_capital,
            cash_available: self.capital,
            total_exposure: exposure,
            unrealized_pnl: Decimal::ZERO, // Simplified
            realized_pnl: Decimal::ZERO,
            current_drawdown: Decimal::ZERO,
            position_count: self.positions.len(),
            last_trade_at: None,
            last_loss_at: None,
        };

        // Calculate size
        let base_size = self.position_sizer.calculate_size(
            trade.amount_usdc,
            dec!(10000),
            self.config.initial_capital,
            None,
            exposure,
        );

        // Validate
        let validation = self.strategy.validate_entry(
            trade.timestamp,
            current_price,
            trade.price,
            base_size,
            None,
            &portfolio,
            &[],
        );

        if !validation.allowed {
            return Ok(Some(format!("Skipped: {}", validation.reason)));
        }

        let size = validation.adjusted_size.unwrap_or(base_size);
        if size <= Decimal::ZERO {
            return Ok(Some("Skipped: Size too small".to_string()));
        }

        // Apply slippage
        let entry_price = match trade.side {
            TradeSide::Buy => current_price * (Decimal::ONE + self.config.slippage),
            TradeSide::Sell => current_price * (Decimal::ONE - self.config.slippage),
        };

        // Calculate cost with fee
        let fee = entry_price * size * self.config.fee_rate;
        let cost = entry_price * size + fee;

        if cost > self.capital {
            return Ok(Some("Skipped: Insufficient capital".to_string()));
        }

        // Execute paper trade
        self.capital -= cost;
        self.total_fees += fee;

        self.positions.insert(position_key, SimulatedPosition {
            market_id: trade.market_id.clone(),
            outcome: trade.outcome.clone(),
            side: trade.side.clone(),
            size,
            entry_price,
            entry_time: Utc::now(),
            source_trader: source_trader.to_string(),
        });

        Ok(None)
    }

    /// Close a position.
    pub fn close_position(
        &mut self,
        position_key: &str,
        exit_price: Decimal,
        reason: &str,
    ) -> Result<Option<String>> {
        let pos = match self.positions.remove(position_key) {
            Some(p) => p,
            None => return Ok(Some("No position to close".to_string())),
        };

        // Apply slippage
        let final_price = match pos.side {
            TradeSide::Buy => exit_price * (Decimal::ONE - self.config.slippage),
            TradeSide::Sell => exit_price * (Decimal::ONE + self.config.slippage),
        };

        let pnl = pos.pnl_at(final_price);
        let return_pct = pos.return_pct(final_price);
        let fee = final_price * pos.size * self.config.fee_rate;
        let net_pnl = pnl - fee;

        self.capital += pos.size * pos.entry_price + net_pnl;
        self.total_fees += fee;

        self.completed_trades.push(BacktestTrade {
            market_id: pos.market_id,
            outcome: pos.outcome,
            side: pos.side,
            size: pos.size,
            entry_price: pos.entry_price,
            exit_price: final_price,
            entry_time: pos.entry_time,
            exit_time: Utc::now(),
            pnl: net_pnl,
            return_pct,
            source_trader: pos.source_trader,
            exit_reason: reason.to_string(),
        });

        Ok(None)
    }

    /// Update equity curve with current prices.
    pub fn update_equity(&mut self, prices: &HashMap<String, Decimal>) {
        let equity = self.current_equity(prices);

        if equity > self.peak_equity {
            self.peak_equity = equity;
        }

        self.equity_curve.push((Utc::now(), equity));
    }

    /// Get current statistics.
    pub fn get_stats(&self, prices: &HashMap<String, Decimal>) -> PaperStats {
        let equity = self.current_equity(prices);
        let unrealized: Decimal = self.positions.iter()
            .map(|(key, pos)| {
                let price = prices.get(key).copied().unwrap_or(pos.entry_price);
                pos.pnl_at(price)
            })
            .sum();

        let realized: Decimal = self.completed_trades.iter().map(|t| t.pnl).sum();

        let winners = self.completed_trades.iter().filter(|t| t.pnl > Decimal::ZERO).count();
        let total = self.completed_trades.len();
        let win_rate = if total > 0 { winners as f64 / total as f64 } else { 0.0 };

        let drawdown = if self.peak_equity > Decimal::ZERO {
            ((self.peak_equity - equity) / self.peak_equity).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };

        PaperStats {
            initial_capital: self.config.initial_capital,
            current_equity: equity,
            cash_available: self.capital,
            unrealized_pnl: unrealized,
            realized_pnl: realized,
            total_pnl: realized + unrealized,
            return_pct: (equity - self.config.initial_capital) / self.config.initial_capital,
            open_positions: self.positions.len(),
            completed_trades: total,
            win_rate,
            max_drawdown: drawdown,
            total_fees: self.total_fees,
            running_since: self.started_at,
        }
    }
}

/// Paper trading statistics.
#[derive(Debug, Clone)]
pub struct PaperStats {
    pub initial_capital: Decimal,
    pub current_equity: Decimal,
    pub cash_available: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub total_pnl: Decimal,
    pub return_pct: Decimal,
    pub open_positions: usize,
    pub completed_trades: usize,
    pub win_rate: f64,
    pub max_drawdown: f64,
    pub total_fees: Decimal,
    pub running_since: DateTime<Utc>,
}

impl std::fmt::Display for PaperStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "\n{:=^50}", " PAPER TRADING ")?;
        writeln!(f, "Running since: {}", self.running_since.format("%Y-%m-%d %H:%M"))?;
        writeln!(f)?;
        writeln!(f, "Initial Capital:  ${:.2}", self.initial_capital)?;
        writeln!(f, "Current Equity:   ${:.2}", self.current_equity)?;
        writeln!(f, "Cash Available:   ${:.2}", self.cash_available)?;
        writeln!(f)?;
        writeln!(f, "Unrealized P&L:   ${:.2}", self.unrealized_pnl)?;
        writeln!(f, "Realized P&L:     ${:.2}", self.realized_pnl)?;
        writeln!(f, "Total P&L:        ${:.2} ({:.2}%)",
            self.total_pnl, self.return_pct * dec!(100))?;
        writeln!(f)?;
        writeln!(f, "Open Positions:   {}", self.open_positions)?;
        writeln!(f, "Completed Trades: {}", self.completed_trades)?;
        writeln!(f, "Win Rate:         {:.1}%", self.win_rate * 100.0)?;
        writeln!(f, "Max Drawdown:     {:.2}%", self.max_drawdown * 100.0)?;
        writeln!(f, "Total Fees:       ${:.2}", self.total_fees)?;
        writeln!(f, "{:=^50}", "")?;
        Ok(())
    }
}
