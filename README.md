# Polymarket Copy-Trading Bot

A Rust-based copy-trading bot that mimics high-value Polymarket traders with intelligent position sizing based on MDD, Sharpe ratio, and other performance metrics.

## Features

- **Trader Discovery**: Find top-performing traders from the Polymarket leaderboard
- **Performance Metrics**: Calculate win rate, Sharpe ratio, Sortino ratio, max drawdown, Calmar ratio
- **Intelligent Position Sizing**: Kelly criterion, fixed fraction, and risk parity methods
- **Composite Scoring**: Rank traders by a weighted combination of metrics
- **Copy-Trading Engine**: Monitor and copy trades from selected traders
- **Persistence**: SQLite database for tracking traders and copy trades

## Installation

```bash
# Clone the repo
git clone https://github.com/U0001F3A2/polymarket-copy.git
cd polymarket-copy

# Build
cargo build --release

# Run
./target/release/polymarket_copier --help
```

## Usage

### Discover Top Traders

```bash
# Find traders with at least $500 profit
polymarket_copier discover --min-pnl 500 --limit 20
```

### Track a Trader

```bash
# Add a trader to your tracking list
polymarket_copier track 0x1234...abcd
```

### View Trader Stats

```bash
# Get detailed metrics for a trader
polymarket_copier stats 0x1234...abcd
```

### List Tracked Traders

```bash
polymarket_copier list
```

### Run the Copy-Trading Bot

```bash
# Start monitoring with $1000 portfolio (dry run mode)
polymarket_copier run --portfolio 1000 --dry-run

# Start actual copy-trading
polymarket_copier run --portfolio 1000
```

## Configuration

The default configuration can be viewed with:

```bash
polymarket_copier config
```

### Trader Selection Criteria

| Metric | Default Threshold |
|--------|-------------------|
| Min Win Rate | 55% |
| Min Trades | 20 |
| Min Profit | $100 |
| Max Drawdown | 40% |
| Min Sharpe | 0.5 |

### Position Sizing

| Parameter | Default |
|-----------|---------|
| Method | Kelly Criterion |
| Kelly Fraction | 25% (quarter Kelly) |
| Max Portfolio Allocation | 50% |
| Max Single Position | 10% |
| Min Trade Size | $1 |
| Max Trade Size | $1000 |

## Architecture

```
src/
├── main.rs           # CLI entry point
├── api/              # Polymarket API client
│   ├── data_client.rs  # Data API for positions/trades
│   └── types.rs        # API response types
├── models/           # Core data models
│   ├── trade.rs        # Trade records
│   ├── trader.rs       # Trader profiles
│   ├── position.rs     # Open positions
│   ├── metrics.rs      # Performance metrics
│   └── market.rs       # Market info
├── metrics/          # Metrics calculation
│   └── calculator.rs   # MDD, Sharpe, win rate, etc.
├── trading/          # Trading logic
│   ├── position_sizer.rs  # Kelly, fixed fraction, risk parity
│   ├── copy_engine.rs     # Copy-trading orchestration
│   └── config.rs          # Trading configuration
└── db/               # Database persistence
    └── mod.rs           # SQLite operations
```

## Metrics Explained

### Composite Score (0-100)

The composite score ranks traders by combining:

- **Win Rate** (25%): Percentage of profitable trades
- **Sharpe Ratio** (25%): Risk-adjusted returns
- **Max Drawdown** (25%): Largest peak-to-trough decline
- **Profitability** (15%): Total P&L
- **Momentum** (10%): Recent 7-day performance

### Position Sizing

**Kelly Criterion**: `f* = (p * b - q) / b`

Where:
- `p` = win rate
- `q` = 1 - p
- `b` = average win / average loss

The bot uses quarter-Kelly (25% of full Kelly) for safety, further adjusted by the trader's max drawdown.

## Disclaimer

This software is for educational purposes only. Trading prediction markets involves significant risk. Past performance does not guarantee future results. Use at your own risk.

## License

MIT
