//! Database persistence for full bot state management.
//!
//! Stores everything needed to resume after restart:
//! - Bot configuration and state
//! - Tracked traders and their metrics
//! - Seen trades (to avoid duplicates)
//! - Our positions and copy trades
//! - Equity curve for P&L tracking

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

/// Database connection pool with full state management.
pub struct Database {
    pool: SqlitePool,
}

/// Bot state stored in database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BotState {
    pub id: i64,
    pub portfolio_value: f64,
    pub current_exposure: f64,
    pub total_pnl: f64,
    pub total_trades: i64,
    pub is_running: bool,
    pub last_poll_at: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

/// Stored position record.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredPosition {
    pub id: i64,
    pub market_id: String,
    pub market_title: String,
    pub outcome: String,
    pub side: String,
    pub size: f64,
    pub entry_price: f64,
    pub current_price: f64,
    pub unrealized_pnl: f64,
    pub source_trader: Option<String>,
    pub opened_at: String,
    pub updated_at: String,
}

/// Stored copy trade record.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredCopyTrade {
    pub id: String,
    pub source_trader: String,
    pub source_trade_id: String,
    pub market_id: String,
    pub market_title: String,
    pub side: String,
    pub outcome: String,
    pub source_size: f64,
    pub source_price: f64,
    pub our_size: f64,
    pub our_price: Option<f64>,
    pub status: String,
    pub order_id: Option<String>,
    pub tx_hash: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub executed_at: Option<String>,
}

/// Equity curve point for tracking P&L over time.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EquityPoint {
    pub id: i64,
    pub timestamp: String,
    pub portfolio_value: f64,
    pub exposure: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
}

impl Database {
    /// Create a new database connection.
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("Failed to connect to database")?;

        let db = Self { pool };
        db.run_migrations().await?;

        Ok(db)
    }

    /// Run all database migrations.
    async fn run_migrations(&self) -> Result<()> {
        // Bot state table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS bot_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                portfolio_value REAL NOT NULL DEFAULT 0,
                current_exposure REAL NOT NULL DEFAULT 0,
                total_pnl REAL NOT NULL DEFAULT 0,
                total_trades INTEGER NOT NULL DEFAULT 0,
                is_running INTEGER NOT NULL DEFAULT 0,
                last_poll_at TEXT,
                started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Tracked traders
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tracked_traders (
                address TEXT PRIMARY KEY,
                pseudonym TEXT,
                profile_image TEXT,
                is_tracked INTEGER NOT NULL DEFAULT 1,
                allocation_weight REAL NOT NULL DEFAULT 1.0,
                last_known_value REAL DEFAULT 0,
                tracking_since TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Trader metrics history
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS trader_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                address TEXT NOT NULL,
                calculated_at TEXT NOT NULL,
                total_trades INTEGER NOT NULL,
                total_volume REAL NOT NULL,
                total_pnl REAL NOT NULL,
                win_rate REAL NOT NULL,
                max_drawdown REAL NOT NULL,
                sharpe_ratio REAL NOT NULL,
                sortino_ratio REAL NOT NULL DEFAULT 0,
                profit_factor REAL NOT NULL DEFAULT 0,
                composite_score REAL NOT NULL,
                FOREIGN KEY (address) REFERENCES tracked_traders(address)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Seen trades (to avoid duplicates)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS seen_trades (
                trade_id TEXT PRIMARY KEY,
                trader_address TEXT NOT NULL,
                market_id TEXT NOT NULL,
                seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Our positions
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                market_id TEXT NOT NULL,
                market_title TEXT NOT NULL DEFAULT '',
                outcome TEXT NOT NULL,
                side TEXT NOT NULL,
                size REAL NOT NULL,
                entry_price REAL NOT NULL,
                current_price REAL NOT NULL DEFAULT 0,
                unrealized_pnl REAL NOT NULL DEFAULT 0,
                source_trader TEXT,
                opened_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                closed_at TEXT,
                UNIQUE(market_id, outcome, side)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Copy trades
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS copy_trades (
                id TEXT PRIMARY KEY,
                source_trader TEXT NOT NULL,
                source_trade_id TEXT NOT NULL,
                market_id TEXT NOT NULL,
                market_title TEXT NOT NULL DEFAULT '',
                side TEXT NOT NULL,
                outcome TEXT NOT NULL,
                source_size REAL NOT NULL,
                source_price REAL NOT NULL,
                our_size REAL NOT NULL,
                our_price REAL,
                status TEXT NOT NULL DEFAULT 'pending',
                order_id TEXT,
                tx_hash TEXT,
                error_message TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                executed_at TEXT,
                FOREIGN KEY (source_trader) REFERENCES tracked_traders(address)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Equity curve
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS equity_curve (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                portfolio_value REAL NOT NULL,
                exposure REAL NOT NULL DEFAULT 0,
                unrealized_pnl REAL NOT NULL DEFAULT 0,
                realized_pnl REAL NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_seen_trades_trader ON seen_trades(trader_address)")
            .execute(&self.pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_copy_trades_status ON copy_trades(status)")
            .execute(&self.pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_positions_market ON positions(market_id)")
            .execute(&self.pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_equity_curve_time ON equity_curve(timestamp)")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ==================== Bot State ====================

    /// Initialize or get bot state.
    pub async fn init_bot_state(&self, portfolio_value: f64) -> Result<BotState> {
        sqlx::query(
            r#"
            INSERT INTO bot_state (id, portfolio_value, is_running, started_at, updated_at)
            VALUES (1, ?, 1, datetime('now'), datetime('now'))
            ON CONFLICT(id) DO UPDATE SET
                portfolio_value = excluded.portfolio_value,
                is_running = 1,
                updated_at = datetime('now')
            "#,
        )
        .bind(portfolio_value)
        .execute(&self.pool)
        .await?;

        self.get_bot_state().await
    }

    /// Get current bot state.
    pub async fn get_bot_state(&self) -> Result<BotState> {
        sqlx::query_as::<_, BotState>("SELECT * FROM bot_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await
            .context("Bot state not initialized")
    }

    /// Update bot state after polling.
    pub async fn update_bot_state(
        &self,
        exposure: f64,
        total_pnl: f64,
        total_trades: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE bot_state SET
                current_exposure = ?,
                total_pnl = ?,
                total_trades = ?,
                last_poll_at = datetime('now'),
                updated_at = datetime('now')
            WHERE id = 1
            "#,
        )
        .bind(exposure)
        .bind(total_pnl)
        .bind(total_trades)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark bot as stopped.
    pub async fn mark_bot_stopped(&self) -> Result<()> {
        sqlx::query("UPDATE bot_state SET is_running = 0, updated_at = datetime('now') WHERE id = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== Traders ====================

    /// Save or update a tracked trader.
    pub async fn save_trader(
        &self,
        address: &str,
        pseudonym: &str,
        allocation_weight: f64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tracked_traders (address, pseudonym, allocation_weight, tracking_since)
            VALUES (?, ?, ?, datetime('now'))
            ON CONFLICT(address) DO UPDATE SET
                pseudonym = COALESCE(NULLIF(excluded.pseudonym, ''), tracked_traders.pseudonym),
                allocation_weight = excluded.allocation_weight,
                is_tracked = 1,
                updated_at = datetime('now')
            "#,
        )
        .bind(address)
        .bind(pseudonym)
        .bind(allocation_weight)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all tracked trader addresses.
    pub async fn get_tracked_addresses(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT address FROM tracked_traders WHERE is_tracked = 1")
                .fetch_all(&self.pool)
                .await?;

        Ok(rows.into_iter().map(|(a,)| a).collect())
    }

    /// Remove a trader from tracking.
    pub async fn remove_trader(&self, address: &str) -> Result<()> {
        sqlx::query(
            "UPDATE tracked_traders SET is_tracked = 0, updated_at = datetime('now') WHERE address = ?",
        )
        .bind(address)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update trader's last known portfolio value.
    pub async fn update_trader_value(&self, address: &str, value: f64) -> Result<()> {
        sqlx::query(
            "UPDATE tracked_traders SET last_known_value = ?, updated_at = datetime('now') WHERE address = ?",
        )
        .bind(value)
        .bind(address)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ==================== Seen Trades ====================

    /// Check if we've already seen a trade.
    pub async fn has_seen_trade(&self, trade_id: &str) -> Result<bool> {
        let result: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM seen_trades WHERE trade_id = ?")
                .bind(trade_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(result.is_some())
    }

    /// Mark a trade as seen.
    pub async fn mark_trade_seen(
        &self,
        trade_id: &str,
        trader_address: &str,
        market_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO seen_trades (trade_id, trader_address, market_id) VALUES (?, ?, ?)",
        )
        .bind(trade_id)
        .bind(trader_address)
        .bind(market_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get count of seen trades for a trader.
    pub async fn get_seen_trade_count(&self, trader_address: &str) -> Result<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM seen_trades WHERE trader_address = ?")
                .bind(trader_address)
                .fetch_one(&self.pool)
                .await?;

        Ok(count)
    }

    // ==================== Positions ====================

    /// Save or update a position.
    pub async fn save_position(
        &self,
        market_id: &str,
        market_title: &str,
        outcome: &str,
        side: &str,
        size: f64,
        entry_price: f64,
        source_trader: Option<&str>,
    ) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO positions (market_id, market_title, outcome, side, size, entry_price, source_trader)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(market_id, outcome, side) DO UPDATE SET
                size = positions.size + excluded.size,
                entry_price = (positions.entry_price * positions.size + excluded.entry_price * excluded.size)
                             / (positions.size + excluded.size),
                updated_at = datetime('now')
            RETURNING id
            "#,
        )
        .bind(market_id)
        .bind(market_title)
        .bind(outcome)
        .bind(side)
        .bind(size)
        .bind(entry_price)
        .bind(source_trader)
        .fetch_one(&self.pool)
        .await?;

        Ok(sqlx::Row::get(&result, "id"))
    }

    /// Get all open positions.
    pub async fn get_open_positions(&self) -> Result<Vec<StoredPosition>> {
        sqlx::query_as::<_, StoredPosition>(
            "SELECT * FROM positions WHERE closed_at IS NULL AND size > 0.0001",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to fetch positions")
    }

    /// Update position price and P&L.
    pub async fn update_position_price(
        &self,
        market_id: &str,
        outcome: &str,
        current_price: f64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE positions SET
                current_price = ?,
                unrealized_pnl = (? - entry_price) * size,
                updated_at = datetime('now')
            WHERE market_id = ? AND outcome = ? AND closed_at IS NULL
            "#,
        )
        .bind(current_price)
        .bind(current_price)
        .bind(market_id)
        .bind(outcome)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Close a position.
    pub async fn close_position(&self, market_id: &str, outcome: &str) -> Result<()> {
        sqlx::query(
            "UPDATE positions SET closed_at = datetime('now'), updated_at = datetime('now') WHERE market_id = ? AND outcome = ?",
        )
        .bind(market_id)
        .bind(outcome)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get total exposure across all positions.
    pub async fn get_total_exposure(&self) -> Result<f64> {
        let (exposure,): (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(size * current_price), 0) FROM positions WHERE closed_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(exposure)
    }

    // ==================== Copy Trades ====================

    /// Save a new copy trade.
    pub async fn save_copy_trade(
        &self,
        id: &str,
        source_trader: &str,
        source_trade_id: &str,
        market_id: &str,
        market_title: &str,
        side: &str,
        outcome: &str,
        source_size: f64,
        source_price: f64,
        our_size: f64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO copy_trades (
                id, source_trader, source_trade_id, market_id, market_title,
                side, outcome, source_size, source_price, our_size, status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending')
            "#,
        )
        .bind(id)
        .bind(source_trader)
        .bind(source_trade_id)
        .bind(market_id)
        .bind(market_title)
        .bind(side)
        .bind(outcome)
        .bind(source_size)
        .bind(source_price)
        .bind(our_size)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update copy trade status.
    pub async fn update_copy_trade_status(
        &self,
        id: &str,
        status: &str,
        order_id: Option<&str>,
        our_price: Option<f64>,
        tx_hash: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let executed_at = if status == "executed" {
            Some("datetime('now')")
        } else {
            None
        };

        sqlx::query(
            r#"
            UPDATE copy_trades SET
                status = ?,
                order_id = COALESCE(?, order_id),
                our_price = COALESCE(?, our_price),
                tx_hash = COALESCE(?, tx_hash),
                error_message = ?,
                executed_at = CASE WHEN ? = 'executed' THEN datetime('now') ELSE executed_at END
            WHERE id = ?
            "#,
        )
        .bind(status)
        .bind(order_id)
        .bind(our_price)
        .bind(tx_hash)
        .bind(error)
        .bind(status)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get pending copy trades.
    pub async fn get_pending_copy_trades(&self) -> Result<Vec<StoredCopyTrade>> {
        sqlx::query_as::<_, StoredCopyTrade>(
            "SELECT * FROM copy_trades WHERE status = 'pending' ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to fetch pending trades")
    }

    /// Get copy trade statistics.
    pub async fn get_copy_trade_stats(&self) -> Result<(i64, i64, i64)> {
        let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM copy_trades")
            .fetch_one(&self.pool)
            .await?;

        let (executed,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM copy_trades WHERE status = 'executed'")
                .fetch_one(&self.pool)
                .await?;

        let (failed,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM copy_trades WHERE status = 'failed'")
                .fetch_one(&self.pool)
                .await?;

        Ok((total, executed, failed))
    }

    // ==================== Equity Curve ====================

    /// Record an equity curve point.
    pub async fn record_equity_point(
        &self,
        portfolio_value: f64,
        exposure: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO equity_curve (portfolio_value, exposure, unrealized_pnl, realized_pnl)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(portfolio_value)
        .bind(exposure)
        .bind(unrealized_pnl)
        .bind(realized_pnl)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get recent equity curve points.
    pub async fn get_equity_curve(&self, limit: i64) -> Result<Vec<EquityPoint>> {
        sqlx::query_as::<_, EquityPoint>(
            "SELECT * FROM equity_curve ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("Failed to fetch equity curve")
    }

    /// Calculate max drawdown from equity curve.
    pub async fn calculate_max_drawdown(&self) -> Result<f64> {
        let points = self.get_equity_curve(1000).await?;

        if points.is_empty() {
            return Ok(0.0);
        }

        let mut peak = 0.0f64;
        let mut max_dd = 0.0f64;

        // Points are in DESC order, reverse for calculation
        for point in points.into_iter().rev() {
            if point.portfolio_value > peak {
                peak = point.portfolio_value;
            }
            if peak > 0.0 {
                let dd = (peak - point.portfolio_value) / peak;
                if dd > max_dd {
                    max_dd = dd;
                }
            }
        }

        Ok(max_dd)
    }

    /// Get the connection pool (for advanced queries).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
