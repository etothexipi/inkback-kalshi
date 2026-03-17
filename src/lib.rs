// Kalshi backtesting modules
pub mod kalshi_types;
pub mod kalshi_csv_io;
pub mod kalshi_event_stream;
pub mod kalshi_l2_book;
pub mod kalshi_strategy;
pub mod kalshi_exec_sim;
pub mod kalshi_backtest;
pub mod kalshi_viz;

// Re-export commonly used items for convenience
pub use kalshi_backtest::{KalshiBacktest, calculate_performance_metrics};
pub use kalshi_strategy::{KalshiStrategy, SpreadMmStrategy, SpreadMmParams, DirectionalStrategy, TrailingMmStrategy, TrailingMmParams};
pub use kalshi_types::{SimConfig, Side}; 