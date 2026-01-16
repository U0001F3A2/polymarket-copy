//! Trading logic: position sizing, copy-trading engine.

mod position_sizer;
mod copy_engine;
mod config;

pub use position_sizer::{PositionSizer, SizingMethod};
pub use copy_engine::CopyEngine;
pub use config::TradingConfig;
