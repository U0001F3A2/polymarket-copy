//! Calculator for trader performance metrics: MDD, Sharpe ratio, win rate, etc.

use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use statrs::statistics::Statistics;

use crate::models::{Trade, TradeSide, TraderMetrics};

/// Calculator for computing trader performance metrics.
pub struct MetricsCalculator;

impl MetricsCalculator {
    /// Calculate comprehensive metrics from a trader's trade history.
    ///
    /// Requires resolved trades (trades where we know the final outcome)
    /// to accurately compute win/loss statistics.
    pub fn calculate(address: &str, trades: &[Trade], resolved_pnls: &[Decimal]) -> TraderMetrics {
        let mut metrics = TraderMetrics::new(address.to_string());

        if trades.is_empty() {
            return metrics;
        }

        metrics.total_trades = trades.len() as u32;

        // Calculate volume
        metrics.total_volume = trades.iter().map(|t| t.amount_usdc).sum();

        // Calculate P&L metrics from resolved trades
        if !resolved_pnls.is_empty() {
            Self::calculate_pnl_metrics(&mut metrics, resolved_pnls);
        }

        // Calculate time-based metrics
        Self::calculate_time_metrics(&mut metrics, trades);

        // Calculate recent performance
        Self::calculate_recent_metrics(&mut metrics, trades, resolved_pnls);

        metrics.calculated_at = Utc::now();
        metrics
    }

    /// Calculate P&L-related metrics from resolved trade outcomes.
    fn calculate_pnl_metrics(metrics: &mut TraderMetrics, pnls: &[Decimal]) {
        let (wins, losses): (Vec<_>, Vec<_>) =
            pnls.iter().partition(|&&p| p > Decimal::ZERO);

        metrics.winning_trades = wins.len() as u32;
        metrics.losing_trades = losses.len() as u32;
        metrics.total_pnl = pnls.iter().copied().sum();

        // Win rate
        if !pnls.is_empty() {
            metrics.win_rate = wins.len() as f64 / pnls.len() as f64;
        }

        // Average win/loss
        if !wins.is_empty() {
            metrics.avg_win = wins.iter().copied().sum::<Decimal>()
                / Decimal::from(wins.len() as u32);
        }
        if !losses.is_empty() {
            metrics.avg_loss = losses.iter().copied().map(|l: Decimal| l.abs()).sum::<Decimal>()
                / Decimal::from(losses.len() as u32);
        }

        // Profit factor
        let gross_profit: Decimal = wins.iter().copied().sum();
        let gross_loss: Decimal = losses.iter().copied().map(|l: Decimal| l.abs()).sum();
        if gross_loss > Decimal::ZERO {
            metrics.profit_factor = gross_profit
                .to_f64()
                .unwrap_or(0.0)
                / gross_loss.to_f64().unwrap_or(1.0);
        }

        // Expectancy
        if !pnls.is_empty() {
            metrics.expectancy = metrics.total_pnl / Decimal::from(pnls.len() as u32);
        }

        // Calculate drawdown and risk metrics
        Self::calculate_drawdown(metrics, pnls);
        Self::calculate_sharpe_sortino(metrics, pnls);
    }

    /// Calculate maximum drawdown from P&L series.
    fn calculate_drawdown(metrics: &mut TraderMetrics, pnls: &[Decimal]) {
        if pnls.is_empty() {
            return;
        }

        // Build equity curve
        let mut equity = Decimal::ZERO;
        let mut peak = Decimal::ZERO;
        let mut max_dd = Decimal::ZERO;
        let mut max_dd_pct = 0.0f64;

        for pnl in pnls {
            equity += pnl;

            if equity > peak {
                peak = equity;
            }

            if peak > Decimal::ZERO {
                let dd = peak - equity;
                if dd > max_dd {
                    max_dd = dd;
                }

                let dd_pct = dd.to_f64().unwrap_or(0.0) / peak.to_f64().unwrap_or(1.0);
                if dd_pct > max_dd_pct {
                    max_dd_pct = dd_pct;
                }
            }
        }

        metrics.max_drawdown = max_dd_pct;
        metrics.max_drawdown_usdc = max_dd;
        metrics.peak_equity = peak;

        // Calmar ratio (annualized return / max drawdown)
        if max_dd_pct > 0.0 && !pnls.is_empty() {
            let total_return = metrics.total_pnl.to_f64().unwrap_or(0.0);
            // Assume trades span roughly a year for simplicity
            // In production, calculate actual time span
            let annualized_return = total_return; // Simplified
            metrics.calmar_ratio = annualized_return / (max_dd_pct * 100.0);
        }
    }

    /// Calculate Sharpe and Sortino ratios.
    fn calculate_sharpe_sortino(metrics: &mut TraderMetrics, pnls: &[Decimal]) {
        if pnls.len() < 2 {
            return;
        }

        let returns: Vec<f64> = pnls
            .iter()
            .filter_map(|p| p.to_f64())
            .collect();

        if returns.is_empty() {
            return;
        }

        let mean = returns.clone().mean();
        let std_dev = returns.clone().std_dev();

        // Sharpe ratio (assuming 0% risk-free rate)
        // Annualized assuming daily returns and 365 trading days
        if std_dev > 0.0 {
            metrics.sharpe_ratio = (mean / std_dev) * (365.0_f64).sqrt();
        }

        // Sortino ratio (using downside deviation)
        let negative_returns: Vec<f64> = returns
            .iter()
            .filter(|&&r| r < 0.0)
            .copied()
            .collect();

        if !negative_returns.is_empty() {
            let downside_dev = negative_returns.std_dev();
            if downside_dev > 0.0 {
                metrics.sortino_ratio = (mean / downside_dev) * (365.0_f64).sqrt();
            }
        }
    }

    /// Calculate time-based metrics.
    fn calculate_time_metrics(metrics: &mut TraderMetrics, trades: &[Trade]) {
        if trades.len() < 2 {
            return;
        }

        // Sort by timestamp
        let mut sorted: Vec<_> = trades.iter().collect();
        sorted.sort_by_key(|t| t.timestamp);

        // Calculate trades per day
        if let (Some(first), Some(last)) = (sorted.first(), sorted.last()) {
            let duration = last.timestamp - first.timestamp;
            let days = duration.num_days().max(1) as f64;
            metrics.trades_per_day = trades.len() as f64 / days;
        }

        // Average holding period would require matching buys/sells
        // Simplified: estimate based on trade frequency
        if metrics.trades_per_day > 0.0 {
            metrics.avg_holding_period_hours = 24.0 / metrics.trades_per_day;
        }
    }

    /// Calculate recent performance metrics (7d, 30d).
    fn calculate_recent_metrics(
        metrics: &mut TraderMetrics,
        trades: &[Trade],
        pnls: &[Decimal],
    ) {
        let now = Utc::now();
        let seven_days_ago = now - Duration::days(7);
        let thirty_days_ago = now - Duration::days(30);

        // Filter recent trades
        let trades_7d: Vec<_> = trades
            .iter()
            .filter(|t| t.timestamp >= seven_days_ago)
            .collect();

        let trades_30d: Vec<_> = trades
            .iter()
            .filter(|t| t.timestamp >= thirty_days_ago)
            .collect();

        // For P&L, we need to correlate trades with pnls
        // Simplified: proportionally estimate based on trade counts
        let total_trades = trades.len();
        if total_trades > 0 && !pnls.is_empty() {
            let ratio_7d = trades_7d.len() as f64 / total_trades as f64;
            let ratio_30d = trades_30d.len() as f64 / total_trades as f64;

            metrics.pnl_7d = metrics.total_pnl * Decimal::try_from(ratio_7d).unwrap_or(Decimal::ZERO);
            metrics.pnl_30d = metrics.total_pnl * Decimal::try_from(ratio_30d).unwrap_or(Decimal::ZERO);
        }

        // Win rates for recent periods (simplified)
        metrics.win_rate_7d = metrics.win_rate; // Would need resolved trades by date
        metrics.win_rate_30d = metrics.win_rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_calculate_pnl_metrics() {
        let pnls = vec![
            dec!(100),   // Win
            dec!(-50),   // Loss
            dec!(200),   // Win
            dec!(-30),   // Loss
            dec!(150),   // Win
        ];

        let trades = vec![]; // Empty for this test
        let metrics = MetricsCalculator::calculate("0x123", &trades, &pnls);

        assert_eq!(metrics.winning_trades, 3);
        assert_eq!(metrics.losing_trades, 2);
        assert_eq!(metrics.total_pnl, dec!(370));
        assert!((metrics.win_rate - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_calculate_drawdown() {
        // Simulate a drawdown scenario
        let pnls = vec![
            dec!(100),   // Equity: 100, Peak: 100
            dec!(50),    // Equity: 150, Peak: 150
            dec!(-80),   // Equity: 70,  Peak: 150, DD: 80 (53%)
            dec!(-20),   // Equity: 50,  Peak: 150, DD: 100 (67%)
            dec!(100),   // Equity: 150, Peak: 150
            dec!(50),    // Equity: 200, Peak: 200
        ];

        let trades = vec![];
        let metrics = MetricsCalculator::calculate("0x123", &trades, &pnls);

        // Max drawdown should be ~67% (100/150)
        assert!(metrics.max_drawdown > 0.65 && metrics.max_drawdown < 0.68);
        assert_eq!(metrics.max_drawdown_usdc, dec!(100));
    }
}
