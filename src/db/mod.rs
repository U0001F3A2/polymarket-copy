//! Database persistence for traders, trades, and metrics.

use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

/// Database connection pool.
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Create a new database connection.
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        let db = Self { pool };
        db.run_migrations().await?;

        Ok(db)
    }

    /// Run database migrations.
    async fn run_migrations(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tracked_traders (
                address TEXT PRIMARY KEY,
                pseudonym TEXT,
                profile_image TEXT,
                is_tracked INTEGER NOT NULL DEFAULT 1,
                tracking_since TEXT,
                allocation_weight REAL NOT NULL DEFAULT 1.0,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

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
                composite_score REAL NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (address) REFERENCES tracked_traders(address)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS copy_trades (
                id TEXT PRIMARY KEY,
                source_trader TEXT NOT NULL,
                source_trade_id TEXT NOT NULL,
                market_id TEXT NOT NULL,
                side TEXT NOT NULL,
                outcome TEXT NOT NULL,
                source_size REAL NOT NULL,
                source_price REAL NOT NULL,
                executed_size REAL,
                executed_price REAL,
                status TEXT NOT NULL DEFAULT 'pending',
                error_message TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                executed_at TEXT,
                FOREIGN KEY (source_trader) REFERENCES tracked_traders(address)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_copy_trades_status ON copy_trades(status)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_trader_metrics_address ON trader_metrics(address)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Save a tracked trader.
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
                pseudonym = excluded.pseudonym,
                allocation_weight = excluded.allocation_weight,
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
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT address FROM tracked_traders WHERE is_tracked = 1",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(a,)| a).collect())
    }

    /// Remove a trader from tracking.
    pub async fn remove_trader(&self, address: &str) -> Result<()> {
        sqlx::query("UPDATE tracked_traders SET is_tracked = 0, updated_at = datetime('now') WHERE address = ?")
            .bind(address)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
