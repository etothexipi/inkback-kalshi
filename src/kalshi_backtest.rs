//! Kalshi Backtesting Module
//! 
//! This module implements a comprehensive backtesting framework for Kalshi prediction markets.
//! It provides event-driven backtesting with L2 orderbook simulation, latency modeling,
//! and maker-only execution.

use crate::kalshi_csv_io::{parse_orderbook, parse_trades};
use crate::kalshi_event_stream::merge;
use crate::kalshi_exec_sim::run_backtest;
use crate::kalshi_strategy::KalshiStrategy;
use crate::kalshi_types::{Report, SimConfig};
use anyhow::Result;

/// High-level interface for running a Kalshi backtest
/// 
/// # Example
/// 
/// ```rust
/// use std::time::Duration;
/// use kalshi_backtest::{KalshiBacktest, SimConfig};
/// use kalshi_backtest::kalshi_strategy::{SpreadMmStrategy, SpreadMmParams};
/// 
/// let config = SimConfig {
///     latency: Duration::from_millis(5),
///     start_time: 0,
///     end_time: None,
/// };
/// 
/// let params = SpreadMmParams::default();
/// let mut strategy = SpreadMmStrategy::new(params);
/// 
/// let backtest = KalshiBacktest::new(config);
/// let result = backtest.run_from_files(
///     "orderbook.csv",
///     "trades.csv", 
///     &mut strategy
/// ).unwrap();
/// 
/// println!("PnL: {} cents", result.metrics.gross_pnl_cents);
/// ```
pub struct KalshiBacktest {
    config: SimConfig,
}

impl KalshiBacktest {
    /// Create a new Kalshi backtest with the given configuration
    pub fn new(config: SimConfig) -> Self {
        Self { config }
    }

    /// Run a backtest using CSV files for orderbook and trades data
    pub fn run_from_files(
        &self,
        orderbook_path: &str,
        trades_path: &str,
        strategy: &mut dyn KalshiStrategy,
    ) -> Result<Report> {
        // Parse CSV files
        let orderbook_iter = parse_orderbook(orderbook_path)?;
        let trades_iter = parse_trades(trades_path)?;

        // Merge into unified event stream
        let events = merge(orderbook_iter, trades_iter);

        // Run backtest
        run_backtest(self.config.clone(), strategy, events)
    }

    /// Run a backtest with pre-loaded events
    pub fn run_with_events<I>(
        &self,
        events: I,
        strategy: &mut dyn KalshiStrategy,
    ) -> Result<Report>
    where
        I: Iterator<Item = Result<crate::kalshi_types::Event>>,
    {
        run_backtest(self.config.clone(), strategy, events)
    }
}

/// Convenience function to run a simple backtest
pub fn run_simple_backtest(
    orderbook_path: &str,
    trades_path: &str,
    strategy: &mut dyn KalshiStrategy,
    latency_ms: u64,
) -> Result<Report> {
    let config = SimConfig {
        latency: std::time::Duration::from_millis(latency_ms),
        start_time: 0,
        end_time: None,
    };

    let backtest = KalshiBacktest::new(config);
    backtest.run_from_files(orderbook_path, trades_path, strategy)
}

/// Calculate basic performance metrics from a report
pub fn calculate_performance_metrics(report: &Report) -> PerformanceMetrics {
    let gross_pnl = report.metrics.gross_pnl_cents as f64 / 100.0; // Convert to dollars
    let total_volume = report.metrics.volume_traded;
    let total_trades = report.metrics.total_fills;
    
    let avg_pnl_per_trade = if total_trades > 0 {
        gross_pnl / total_trades as f64
    } else {
        0.0
    };

    let avg_pnl_per_contract = if total_volume > 0 {
        gross_pnl / total_volume as f64
    } else {
        0.0
    };

    PerformanceMetrics {
        gross_pnl_dollars: gross_pnl,
        total_volume,
        total_trades,
        avg_pnl_per_trade,
        avg_pnl_per_contract,
        win_rate: report.metrics.win_rate,
        num_spreads: report.metrics.num_spreads,
        roi: report.metrics.roi,
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub gross_pnl_dollars: f64,
    pub total_volume: i64,
    pub total_trades: usize,
    pub avg_pnl_per_trade: f64,
    pub avg_pnl_per_contract: f64,
    pub win_rate: f64,
    pub num_spreads: usize,
    pub roi: f64,
}

impl std::fmt::Display for PerformanceMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, 
            "Performance Metrics:\n\
             Gross PnL: ${:.2}\n\
             Total Volume: {} contracts\n\
             Total Trades: {}\n\
             Avg PnL per Trade: ${:.4}\n\
             Avg PnL per Contract: ${:.4}\n\
             Win Rate: {:.1}%\n\
             Completed Spreads: {}\n\
             ROI: {:.2}%",
            self.gross_pnl_dollars,
            self.total_volume,
            self.total_trades,
            self.avg_pnl_per_trade,
            self.avg_pnl_per_contract,
            self.win_rate * 100.0,
            self.num_spreads,
            self.roi * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kalshi_strategy::{SpreadMmStrategy, SpreadMmParams};
    use crate::kalshi_types::{Event, Side};
    use std::time::Duration;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_csv_files() -> Result<(NamedTempFile, NamedTempFile)> {
        // Create orderbook CSV
        let mut orderbook_file = NamedTempFile::new()?;
        writeln!(orderbook_file, "seq,client_ts,ticker,msg_type,yes_levels,no_levels")?;
        writeln!(orderbook_file, "1,1000,TEST-CONTRACT,snapshot,\"[{{\"price\":45,\"quantity\":100}}]\",\"[{{\"price\":55,\"quantity\":100}}]\"")?;
        writeln!(orderbook_file, "3,3000,TEST-CONTRACT,snapshot,\"[{{\"price\":46,\"quantity\":150}}]\",\"[{{\"price\":54,\"quantity\":150}}]\"")?;

        // Create trades CSV
        let mut trades_file = NamedTempFile::new()?;
        writeln!(trades_file, "seq,client_ts,ticker,yes_price,no_price,quantity,taker")?;
        writeln!(trades_file, "2,2000,TEST-CONTRACT,46,54,25,yes")?;
        writeln!(trades_file, "4,4000,TEST-CONTRACT,46,54,35,no")?;

        orderbook_file.flush()?;
        trades_file.flush()?;

        Ok((orderbook_file, trades_file))
    }

    #[test]
    fn test_kalshi_backtest_integration() -> Result<()> {
        let (orderbook_file, trades_file) = create_test_csv_files()?;

        let config = SimConfig {
            latency: Duration::from_millis(1),
            start_time: 0,
            end_time: None,
        };

        let mut strategy = SpreadMmStrategy::new(SpreadMmParams::default());
        let backtest = KalshiBacktest::new(config);

        let result = backtest.run_from_files(
            orderbook_file.path().to_str().unwrap(),
            trades_file.path().to_str().unwrap(),
            &mut strategy,
        )?;

        // Should have processed some events
        assert!(result.metrics.total_orders > 0);
        
        // Calculate performance metrics
        let perf = calculate_performance_metrics(&result);
        println!("{}", perf);

        Ok(())
    }

    #[test]
    fn test_run_simple_backtest() -> Result<()> {
        let (orderbook_file, trades_file) = create_test_csv_files()?;

        let mut strategy = SpreadMmStrategy::new(SpreadMmParams {
            min_spread: 2,
            post_qty: 50,
            max_pos: 200,
            order_ttl: Duration::from_secs(5),
        });

        let result = run_simple_backtest(
            orderbook_file.path().to_str().unwrap(),
            trades_file.path().to_str().unwrap(),
            &mut strategy,
            5, // 5ms latency
        )?;

        assert!(result.orders.len() > 0);
        
        Ok(())
    }

    #[test]
    fn test_performance_metrics_calculation() {
        let report = Report {
            metrics: crate::kalshi_types::Metrics {
                gross_pnl_cents: 250, // $2.50
                num_spreads: 5,
                volume_traded: 100,
                win_rate: 0.6,
                roi: 0.15,
                total_fills: 10,
                total_orders: 20,
                cancelled_orders: 5,
                expired_orders: 3,
            },
            fills: vec![],
            orders: vec![],
        };

        let perf = calculate_performance_metrics(&report);
        
        assert_eq!(perf.gross_pnl_dollars, 2.5);
        assert_eq!(perf.total_volume, 100);
        assert_eq!(perf.total_trades, 10);
        assert_eq!(perf.avg_pnl_per_trade, 0.25);
        assert_eq!(perf.avg_pnl_per_contract, 0.025);
        assert_eq!(perf.win_rate, 0.6);
        assert_eq!(perf.num_spreads, 5);
        assert_eq!(perf.roi, 0.15);
    }
} 