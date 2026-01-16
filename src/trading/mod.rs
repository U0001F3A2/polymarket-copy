//! Trading logic: position sizing, copy-trading engine, strategy.

mod config;
mod copy_engine;
mod position_sizer;
mod strategy;

pub use config::TradingConfig;
pub use copy_engine::{CopyEngine, CopyTradeIntent, EngineStats};
pub use position_sizer::{PositionSizer, SizingMethod};
pub use strategy::{
    EntryValidation, ExitReason, ExitSignal, ExitUrgency, PortfolioState, PositionRisk,
    Strategy, StrategyConfig, StrategyPosition,
};
