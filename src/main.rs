//! Polymarket Copy-Trading Bot
//!
//! Mimics high-value traders with intelligent position sizing based on
//! MDD, Sharpe ratio, and other performance metrics.

mod api;
mod backtest;
mod bot;
mod db;
mod metrics;
mod models;
mod trading;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use crate::backtest::{BacktestConfig, Backtester, PaperConfig, PaperTrader};
use crate::bot::{Bot, BotConfig};
use crate::db::Database;
use crate::trading::{CopyEngine, StrategyConfig, TradingConfig};

/// Polymarket copy-trading bot CLI.
#[derive(Parser)]
#[command(name = "polycopier")]
#[command(about = "Copy trades from successful Polymarket traders", long_about = None)]
struct Cli {
    /// Database file path
    #[arg(short, long, default_value = "sqlite:./polycopier.db?mode=rwc")]
    database: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover top traders from the leaderboard
    Discover {
        /// Minimum P&L in USD to filter traders
        #[arg(short, long, default_value = "500")]
        min_pnl: f64,

        /// Maximum number of traders to show
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Time period (DAY, WEEK, MONTH, ALL)
        #[arg(short, long, default_value = "MONTH")]
        period: String,
    },

    /// Add a trader to track
    Track {
        /// Trader's wallet address
        address: String,
    },

    /// Remove a trader from tracking
    Untrack {
        /// Trader's wallet address
        address: String,
    },

    /// List all tracked traders
    List,

    /// Show detailed stats for a trader
    Stats {
        /// Trader's wallet address
        address: String,
    },

    /// Start the copy-trading bot
    Run {
        /// Your portfolio value in USDC
        #[arg(short, long)]
        portfolio: f64,

        /// Polling interval in seconds
        #[arg(short, long, default_value = "30")]
        interval: u64,

        /// Dry run (don't execute trades)
        #[arg(long)]
        dry_run: bool,
    },

    /// Show current configuration
    Config,

    /// Show bot status and statistics
    Status,

    /// Run historical backtest on a trader's trades
    Backtest {
        /// Trader address to backtest
        #[arg(short, long)]
        trader: Option<String>,

        /// Initial capital for simulation
        #[arg(short, long, default_value = "10000")]
        capital: f64,

        /// Number of historical trades to analyze
        #[arg(short, long, default_value = "500")]
        lookback: u32,

        /// Simulated slippage percentage (0-100)
        #[arg(long, default_value = "0.5")]
        slippage: f64,

        /// Simulated fee percentage (0-100)
        #[arg(long, default_value = "0.1")]
        fee: f64,

        /// Backtest all tracked traders
        #[arg(long)]
        all: bool,
    },

    /// Start paper trading (simulated live trading)
    Paper {
        /// Initial capital for simulation
        #[arg(short, long, default_value = "10000")]
        capital: f64,

        /// Polling interval in seconds
        #[arg(short, long, default_value = "30")]
        interval: u64,

        /// Simulated slippage percentage (0-100)
        #[arg(long, default_value = "0.3")]
        slippage: f64,

        /// Simulated fee percentage (0-100)
        #[arg(long, default_value = "0.1")]
        fee: f64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging
    let log_level = match cli.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Initialize database
    let db = Database::new(&cli.database).await?;

    // Initialize copy engine
    let config = TradingConfig::default();
    let engine = CopyEngine::new(config)?;

    match cli.command {
        Commands::Discover {
            min_pnl,
            limit,
            period: _,
        } => {
            info!("Discovering top traders with min P&L ${}", min_pnl);

            let traders = engine.discover_traders(min_pnl, limit).await?;

            println!("\n{:<44} {:<20} {:>10}", "ADDRESS", "NAME", "SCORE");
            println!("{}", "-".repeat(76));

            for trader in traders {
                let score = trader.score();
                println!(
                    "{:<44} {:<20} {:>10.1}",
                    trader.address,
                    truncate(&trader.display_name(), 18),
                    score
                );
            }
        }

        Commands::Track { address } => {
            info!(address = %address, "Adding trader to tracking");

            engine.add_trader(address.clone()).await?;
            db.save_trader(&address, "", 1.0).await?;

            println!("Now tracking: {}", address);

            // Show trader stats
            let traders = engine.get_tracked_traders().await;
            if let Some(trader) = traders.iter().find(|t| t.address == address) {
                if let Some(m) = &trader.metrics {
                    println!("\nMetrics:");
                    println!("  Win Rate:     {:.1}%", m.win_rate * 100.0);
                    println!("  Sharpe Ratio: {:.2}", m.sharpe_ratio);
                    println!("  Max Drawdown: {:.1}%", m.max_drawdown * 100.0);
                    println!("  Total Trades: {}", m.total_trades);
                    println!("  Score:        {:.1}", m.composite_score());
                }
            }
        }

        Commands::Untrack { address } => {
            engine.remove_trader(&address).await;
            db.remove_trader(&address).await?;
            println!("Stopped tracking: {}", address);
        }

        Commands::List => {
            let addresses = db.get_tracked_addresses().await?;

            if addresses.is_empty() {
                println!("No traders being tracked. Use 'polycopier track <address>' to add one.");
                return Ok(());
            }

            // Load traders into engine
            for addr in &addresses {
                let _ = engine.add_trader(addr.clone()).await;
            }

            let traders = engine.get_tracked_traders().await;

            println!(
                "\n{:<44} {:<12} {:>8} {:>8} {:>10}",
                "ADDRESS", "NAME", "WIN%", "SHARPE", "SCORE"
            );
            println!("{}", "-".repeat(86));

            for trader in traders {
                let (win_rate, sharpe, score) = trader
                    .metrics
                    .as_ref()
                    .map(|m| (m.win_rate * 100.0, m.sharpe_ratio, m.composite_score()))
                    .unwrap_or((0.0, 0.0, 0.0));

                println!(
                    "{:<44} {:<12} {:>7.1}% {:>8.2} {:>10.1}",
                    trader.address,
                    truncate(&trader.display_name(), 10),
                    win_rate,
                    sharpe,
                    score
                );
            }
        }

        Commands::Stats { address } => {
            engine.add_trader(address.clone()).await?;

            let traders = engine.get_tracked_traders().await;
            let trader = traders
                .iter()
                .find(|t| t.address == address)
                .ok_or_else(|| anyhow::anyhow!("Trader not found"))?;

            println!("\n=== Trader: {} ===", trader.display_name());
            println!("Address: {}", trader.address);

            if let Some(m) = &trader.metrics {
                println!("\n--- Performance Metrics ---");
                println!("Total Trades:   {}", m.total_trades);
                println!("Total Volume:   ${:.2}", m.total_volume);
                println!("Total P&L:      ${:.2}", m.total_pnl);

                println!("\n--- Win/Loss ---");
                println!("Win Rate:       {:.1}%", m.win_rate * 100.0);
                println!("Winning Trades: {}", m.winning_trades);
                println!("Losing Trades:  {}", m.losing_trades);
                println!("Avg Win:        ${:.2}", m.avg_win);
                println!("Avg Loss:       ${:.2}", m.avg_loss);
                println!("Profit Factor:  {:.2}", m.profit_factor);

                println!("\n--- Risk Metrics ---");
                println!("Max Drawdown:   {:.1}%", m.max_drawdown * 100.0);
                println!("Sharpe Ratio:   {:.2}", m.sharpe_ratio);
                println!("Sortino Ratio:  {:.2}", m.sortino_ratio);
                println!("Calmar Ratio:   {:.2}", m.calmar_ratio);

                println!("\n--- Scoring ---");
                println!("Composite Score:      {:.1}/100", m.composite_score());
                println!(
                    "Suggested Allocation: {:.1}%",
                    m.suggested_allocation() * 100.0
                );
                println!(
                    "Quality Trader:       {}",
                    if m.is_quality_trader() { "Yes" } else { "No" }
                );
            }

            println!("\n--- Open Positions ({}) ---", trader.positions.len());
            for pos in &trader.positions {
                println!(
                    "  {} {} @ {:.3} (P&L: ${:.2})",
                    pos.market_title, pos.outcome, pos.average_price, pos.unrealized_pnl
                );
            }
        }

        Commands::Run {
            portfolio,
            interval,
            dry_run,
        } => {
            info!(
                portfolio = portfolio,
                interval = interval,
                dry_run = dry_run,
                "Starting copy-trading bot"
            );

            // Check for tracked traders first
            let addresses = db.get_tracked_addresses().await?;
            if addresses.is_empty() {
                println!("No traders being tracked. Use 'polycopier track <address>' first.");
                return Ok(());
            }

            // Configure the bot
            let bot_config = BotConfig {
                portfolio_value: Decimal::try_from(portfolio)?,
                poll_interval_secs: interval,
                dry_run,
                trading_config: TradingConfig::default(),
                strategy_config: StrategyConfig::default(),
                database_url: cli.database.clone(),
            };

            // Create and initialize the bot
            let mut bot = Bot::new(bot_config).await?;
            bot.initialize().await?;

            println!("\n=== Polymarket Copy-Trading Bot ===");
            println!("Portfolio value: ${}", portfolio);
            println!("Polling interval: {}s", interval);
            println!("Mode: {}", if dry_run { "DRY RUN (no real trades)" } else { "LIVE TRADING" });
            println!("Tracked traders: {}", addresses.len());
            println!("\nPress Ctrl+C to stop.\n");

            // Run the bot
            if let Err(e) = bot.run().await {
                tracing::error!(error = %e, "Bot error");
            }

            // Show final stats
            let stats = bot.get_stats().await;
            println!("\n{}", stats);
        }

        Commands::Config => {
            let config = TradingConfig::default();
            let strategy = StrategyConfig::default();

            println!("\n=== Trading Configuration ===\n");
            println!("Position Sizing:");
            println!("  Method:               {}", config.sizing_method);
            println!("  Kelly Fraction:       {}", config.kelly_fraction);
            println!("  Max Portfolio Alloc:  {}%", config.max_portfolio_allocation * Decimal::from(100));
            println!("  Max Single Position:  {}%", config.max_single_position * Decimal::from(100));
            println!("  Min Trade Size:       ${}", config.min_trade_size);
            println!("  Max Trade Size:       ${}", config.max_trade_size);

            println!("\nRisk Management:");
            println!("  Max Drawdown:         {}%", config.max_drawdown_pct * Decimal::from(100));
            println!("  Slippage Tolerance:   {}%", config.slippage_tolerance * Decimal::from(100));

            println!("\nTrader Requirements:");
            println!("  Min Win Rate:         {:.0}%", config.min_win_rate * 100.0);
            println!("  Min Trades:           {}", config.min_trades);
            println!("  Min Profit:           ${}", config.min_profit);
            println!("  Max Trader MDD:       {:.0}%", config.max_trader_mdd * 100.0);
            println!("  Min Sharpe:           {:.1}", config.min_sharpe);

            println!("\n=== Strategy Configuration ===\n");
            println!("Entry Rules:");
            println!("  Max Trade Age:        {}s", strategy.max_trade_age_secs);
            println!("  Min Entry Price:      {}", strategy.min_entry_price);
            println!("  Max Entry Price:      {}", strategy.max_entry_price);
            println!("  Max Entry Slippage:   {}%", strategy.max_entry_slippage * dec!(100));
            println!("  Min Trader Score:     {}", strategy.min_trader_score);

            println!("\nExit Rules:");
            println!("  Take Profit:          {}%", strategy.take_profit_pct * dec!(100));
            println!("  Stop Loss:            {}%", strategy.stop_loss_pct * dec!(100));
            println!("  Max Holding Period:   {}h", strategy.max_holding_hours);
            println!("  Follow Trader Exits:  {}", strategy.follow_trader_exits);

            println!("\nPortfolio Risk:");
            println!("  Max Drawdown:         {}%", strategy.max_portfolio_drawdown * dec!(100));
            println!("  Max Positions:        {}", strategy.max_concurrent_positions);
            println!("  Max Single Market:    {}%", strategy.max_single_market_exposure * dec!(100));
        }

        Commands::Status => {
            // Load bot state from database
            let bot_state = match db.get_bot_state().await {
                Ok(state) => state,
                Err(_) => {
                    println!("No bot session found. Run 'polycopier run' to start the bot.");
                    return Ok(());
                }
            };

            let (total, executed, failed) = db.get_copy_trade_stats().await.unwrap_or((0, 0, 0));
            let max_dd = db.calculate_max_drawdown().await.unwrap_or(0.0);
            let addresses = db.get_tracked_addresses().await?;
            let positions = db.get_open_positions().await?;

            println!("\n=== Bot Status ===");
            println!("Running:          {}", if bot_state.is_running { "Yes" } else { "No" });
            println!("Started:          {}", bot_state.started_at);
            println!("Last Poll:        {}", bot_state.last_poll_at.unwrap_or_else(|| "Never".to_string()));

            println!("\n=== Portfolio ===");
            println!("Value:            ${:.2}", bot_state.portfolio_value);
            println!("Exposure:         ${:.2}", bot_state.current_exposure);
            println!("Total P&L:        ${:.2}", bot_state.total_pnl);
            println!("Max Drawdown:     {:.2}%", max_dd * 100.0);

            println!("\n=== Trading ===");
            println!("Tracked Traders:  {}", addresses.len());
            println!("Open Positions:   {}", positions.len());
            println!("Total Trades:     {}", total);
            println!("Executed:         {}", executed);
            println!("Failed:           {}", failed);

            if !positions.is_empty() {
                println!("\n=== Open Positions ===");
                for pos in &positions {
                    let pnl_sign = if pos.unrealized_pnl >= 0.0 { "+" } else { "" };
                    println!(
                        "  {} {} @ {:.3} -> {:.3} ({}${:.2})",
                        truncate(&pos.market_id, 20),
                        pos.outcome,
                        pos.entry_price,
                        pos.current_price,
                        pnl_sign,
                        pos.unrealized_pnl
                    );
                }
            }
        }

        Commands::Backtest {
            trader,
            capital,
            lookback,
            slippage,
            fee,
            all,
        } => {
            info!(
                capital = capital,
                lookback = lookback,
                slippage = slippage,
                fee = fee,
                "Starting backtest"
            );

            // Build backtest config
            let backtest_config = BacktestConfig {
                initial_capital: Decimal::try_from(capital)?,
                trading_config: TradingConfig::default(),
                strategy_config: StrategyConfig::default(),
                slippage: Decimal::try_from(slippage / 100.0)?,
                fee_rate: Decimal::try_from(fee / 100.0)?,
                lookback_trades: lookback,
            };

            let backtester = Backtester::new(backtest_config)?;

            if all {
                // Backtest all tracked traders
                let addresses = db.get_tracked_addresses().await?;
                if addresses.is_empty() {
                    println!("No traders being tracked. Use 'polycopier track <address>' first.");
                    return Ok(());
                }

                println!("\n=== Multi-Trader Backtest ===");
                println!("Capital: ${}", capital);
                println!("Traders: {}", addresses.len());
                println!("Lookback: {} trades per trader", lookback);
                println!("Slippage: {}%", slippage);
                println!("Fee: {}%", fee);
                println!("\nFetching historical data...\n");

                let results = backtester.run_multiple_traders(&addresses).await?;
                println!("{}", results);

                // Show top trades
                if !results.trades.is_empty() {
                    println!("\n--- Top 5 Winning Trades ---");
                    let mut sorted = results.trades.clone();
                    sorted.sort_by(|a, b| b.pnl.partial_cmp(&a.pnl).unwrap_or(std::cmp::Ordering::Equal));
                    for trade in sorted.iter().take(5) {
                        println!(
                            "  {} {} | P&L: ${:.2} ({:.1}%)",
                            truncate(&trade.market_id, 25),
                            trade.outcome,
                            trade.pnl,
                            trade.return_pct * dec!(100)
                        );
                    }

                    println!("\n--- Top 5 Losing Trades ---");
                    for trade in sorted.iter().rev().take(5) {
                        if trade.pnl < Decimal::ZERO {
                            println!(
                                "  {} {} | P&L: ${:.2} ({:.1}%)",
                                truncate(&trade.market_id, 25),
                                trade.outcome,
                                trade.pnl,
                                trade.return_pct * dec!(100)
                            );
                        }
                    }
                }
            } else if let Some(address) = trader {
                // Backtest single trader
                println!("\n=== Single Trader Backtest ===");
                println!("Trader: {}", address);
                println!("Capital: ${}", capital);
                println!("Lookback: {} trades", lookback);
                println!("Slippage: {}%", slippage);
                println!("Fee: {}%", fee);
                println!("\nFetching historical data...\n");

                let results = backtester.run_single_trader(&address).await?;
                println!("{}", results);

                // Show trade breakdown by exit reason
                let mut by_reason: std::collections::HashMap<String, (usize, Decimal)> = std::collections::HashMap::new();
                for trade in &results.trades {
                    let entry = by_reason.entry(trade.exit_reason.clone()).or_insert((0, Decimal::ZERO));
                    entry.0 += 1;
                    entry.1 += trade.pnl;
                }

                println!("\n--- Trades by Exit Reason ---");
                for (reason, (count, pnl)) in by_reason {
                    println!("  {:<20} {:>3} trades  ${:.2}", reason, count, pnl);
                }
            } else {
                println!("Please specify a trader address with --trader or use --all to backtest all tracked traders.");
                println!("\nExamples:");
                println!("  polycopier backtest --trader 0x123...");
                println!("  polycopier backtest --all --capital 5000");
            }
        }

        Commands::Paper {
            capital,
            interval,
            slippage,
            fee,
        } => {
            info!(
                capital = capital,
                interval = interval,
                "Starting paper trading"
            );

            // Check for tracked traders
            let addresses = db.get_tracked_addresses().await?;
            if addresses.is_empty() {
                println!("No traders being tracked. Use 'polycopier track <address>' first.");
                return Ok(());
            }

            // Configure paper trader
            let paper_config = PaperConfig {
                initial_capital: Decimal::try_from(capital)?,
                trading_config: TradingConfig::default(),
                strategy_config: StrategyConfig::default(),
                slippage: Decimal::try_from(slippage / 100.0)?,
                fee_rate: Decimal::try_from(fee / 100.0)?,
            };

            let mut paper_trader = PaperTrader::new(paper_config);

            println!("\n=== Paper Trading Mode ===");
            println!("Capital: ${}", capital);
            println!("Polling interval: {}s", interval);
            println!("Slippage: {}%", slippage);
            println!("Fee: {}%", fee);
            println!("Tracked traders: {}", addresses.len());
            println!("\nThis is SIMULATED trading - no real money involved.");
            println!("Press Ctrl+C to stop.\n");

            // Load traders into engine
            for addr in &addresses {
                let _ = engine.add_trader(addr.clone()).await;
            }

            // Paper trading loop
            let mut last_poll = std::time::Instant::now();
            let poll_duration = std::time::Duration::from_secs(interval);

            loop {
                if last_poll.elapsed() >= poll_duration {
                    // Fetch new trades from tracked traders
                    for addr in &addresses {
                        if let Ok(trades) = engine.get_trader_trades(addr, Some(10), None).await {
                            for trade in trades {
                                // Process each trade through paper trader
                                match paper_trader.process_trade(&trade, addr, trade.price) {
                                    Ok(Some(msg)) => {
                                        tracing::debug!(msg = %msg, "Paper trade skipped");
                                    }
                                    Ok(None) => {
                                        info!(
                                            market = %trade.market_id,
                                            side = ?trade.side,
                                            "Paper trade executed"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Paper trade error");
                                    }
                                }
                            }
                        }
                    }

                    // Update equity with current prices (simplified - use entry prices)
                    let prices: std::collections::HashMap<String, Decimal> = paper_trader
                        .positions
                        .iter()
                        .map(|(k, p)| (k.clone(), p.entry_price))
                        .collect();
                    paper_trader.update_equity(&prices);

                    // Show status
                    let stats = paper_trader.get_stats(&prices);
                    println!(
                        "[{}] Equity: ${:.2} | P&L: ${:.2} ({:.2}%) | Positions: {} | Trades: {}",
                        chrono::Local::now().format("%H:%M:%S"),
                        stats.current_equity,
                        stats.total_pnl,
                        stats.return_pct * dec!(100),
                        stats.open_positions,
                        stats.completed_trades
                    );

                    last_poll = std::time::Instant::now();
                }

                // Check for Ctrl+C
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        println!("\n\nStopping paper trading...");
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }

            // Show final stats
            let prices: std::collections::HashMap<String, Decimal> = paper_trader
                .positions
                .iter()
                .map(|(k, p)| (k.clone(), p.entry_price))
                .collect();
            let stats = paper_trader.get_stats(&prices);
            println!("{}", stats);
        }
    }

    Ok(())
}

/// Truncate a string with ellipsis if too long.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
