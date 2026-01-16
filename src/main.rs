//! Polymarket Copy-Trading Bot
//!
//! Mimics high-value traders with intelligent position sizing based on
//! MDD, Sharpe ratio, and other performance metrics.

mod api;
mod db;
mod metrics;
mod models;
mod trading;

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use crate::db::Database;
use crate::trading::{CopyEngine, TradingConfig};

/// Polymarket copy-trading bot CLI.
#[derive(Parser)]
#[command(name = "polycopier")]
#[command(about = "Copy trades from successful Polymarket traders", long_about = None)]
struct Cli {
    /// Database file path
    #[arg(short, long, default_value = "sqlite:polycopier.db")]
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

            // Load tracked traders
            let addresses = db.get_tracked_addresses().await?;
            if addresses.is_empty() {
                println!("No traders being tracked. Use 'polycopier track <address>' first.");
                return Ok(());
            }

            for addr in &addresses {
                engine.add_trader(addr.clone()).await?;
            }

            engine
                .set_portfolio_value(Decimal::try_from(portfolio)?)
                .await;

            println!("Loaded {} tracked traders", addresses.len());
            println!("Portfolio value: ${}", portfolio);
            println!("Polling interval: {}s", interval);
            println!("Dry run: {}", dry_run);
            println!("\nStarting trade monitoring...\n");

            // Main loop
            loop {
                match engine.poll_for_trades().await {
                    Ok(intents) => {
                        for intent in intents {
                            println!(
                                "[{}] Copy trade: {} {} {} ${:.2} -> Our size: ${:.2}",
                                intent.created_at.format("%H:%M:%S"),
                                intent.source_trade.market_title,
                                intent.source_trade.side.as_str(),
                                intent.source_trade.outcome,
                                intent.source_trade.amount_usdc,
                                intent.calculated_size
                            );

                            if !dry_run {
                                // TODO: Execute trade via CLOB client
                                println!("  -> Would execute trade (CLOB integration pending)");
                            } else {
                                println!("  -> Dry run, not executing");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Error polling for trades");
                    }
                }

                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        }

        Commands::Config => {
            let config = TradingConfig::default();
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
            println!("  Min Win Rate:         {}%", config.min_win_rate * 100.0);
            println!("  Min Trades:           {}", config.min_trades);
            println!("  Min Profit:           ${}", config.min_profit);
            println!("  Max Trader MDD:       {}%", config.max_trader_mdd * 100.0);
            println!("  Min Sharpe:           {}", config.min_sharpe);
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
