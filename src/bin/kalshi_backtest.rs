use clap::{Arg, Command};
use anyhow::Result;
use std::time::Duration;
use std::path::{Path, PathBuf};
use std::fs;
use std::collections::HashSet;
use csv::ReaderBuilder;

use InkBack::{KalshiBacktest, calculate_performance_metrics, SpreadMmStrategy, SpreadMmParams, DirectionalStrategy, TrailingMmStrategy, TrailingMmParams, SimConfig, Side};
use InkBack::{kalshi_types::*, kalshi_strategy::KalshiStrategy, kalshi_csv_io::*, kalshi_event_stream::*, kalshi_exec_sim::run_backtest};
use std::rc::Rc;
use std::cell::RefCell;
use std::io::Write;

#[derive(Debug, Clone)]
struct MarketResult {
    market_ticker: String,
    report: InkBack::kalshi_types::Report,
    performance: InkBack::kalshi_backtest::PerformanceMetrics,
}

#[derive(Debug)]
struct AggregatedMetrics {
    total_markets: usize,
    profitable_markets: usize,
    total_gross_pnl: f64,
    total_volume: i64,
    total_trades: usize,
    total_orders: usize,
    total_fills: usize,
    total_cancelled: usize,
    total_expired: usize,
    avg_win_rate: f64,
    best_market: Option<String>,
    worst_market: Option<String>,
    best_pnl: f64,
    worst_pnl: f64,
}

fn main() -> Result<()> {
    let matches = Command::new("kalshi_backtest")
        .about("Run Kalshi backtests with real CSV data")
        .arg(
            Arg::new("data")
                .long("data")
                .short('d')
                .value_name("DIR")
                .help("Path to data directory containing orderbook*.csv and trades*.csv files")
                .required(true),
        )
        .arg(
            Arg::new("pattern")
                .long("pattern")
                .short('p')
                .value_name("PATTERN")
                .help("File pattern to match (e.g., '20250723_135706' to match specific timestamp)")
                .required(false),
        )
        .arg(
            Arg::new("market")
                .long("market")
                .short('m')
                .value_name("TICKER")
                .help("Specific market ticker to backtest (e.g., 'KXMLBGAME-25JUL23SDMIA-SD')")
                .required(false),
        )
        .arg(
            Arg::new("prefix")
                .long("prefix")
                .value_name("PREFIX")
                .help("Market ticker prefix to filter markets (e.g., 'KXMLBGAME' to test all MLB games)")
                .required(false),
        )
        .arg(
            Arg::new("strategy")
                .long("strategy")
                .short('s')
                .value_name("STRATEGY")
                .help("Strategy type: spread_mm, trailing_spread_mm, directional_yes, directional_no")
                .default_value("spread_mm"),
        )
        .arg(
            Arg::new("latency")
                .long("latency")
                .short('l')
                .value_name("MS")
                .help("Latency in milliseconds")
                .default_value("5"),
        )
        .arg(
            Arg::new("min-spread")
                .long("min-spread")
                .value_name("CENTS")
                .help("Minimum spread for market making (cents)")
                .default_value("3"),
        )
        .arg(
            Arg::new("qty")
                .long("qty")
                .short('q')
                .value_name("SIZE")
                .help("Order quantity")
                .default_value("50"),
        )
        .arg(
            Arg::new("max-pos")
                .long("max-pos")
                .value_name("SIZE")
                .help("Maximum position size")
                .default_value("200"),
        )
        .arg(
            Arg::new("list-files")
                .long("list-files")
                .help("List available data file pairs and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("list-markets")
                .long("list-markets")
                .help("List available market tickers in the data files and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("all-markets")
                .long("all-markets")
                .help("Run backtest on all markets found in the data files")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("summary-only")
                .long("summary-only")
                .help("Show only aggregated summary, skip individual market details")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("debug-log")
                .long("debug-log")
                .help("Enable debug logging to kalshi_debug.log with chronological event details")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("pnl-log")
                .long("pnl-log")
                .help("Enable PnL logging to kalshi_pnl.log with step-by-step PnL calculations")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let data_dir = matches.get_one::<String>("data").unwrap();
    let pattern = matches.get_one::<String>("pattern");
    let target_market = matches.get_one::<String>("market");
    let market_prefix = matches.get_one::<String>("prefix");
    let strategy_type = matches.get_one::<String>("strategy").unwrap();
    let latency_ms: u64 = matches.get_one::<String>("latency").unwrap().parse()?;
    let min_spread: u8 = matches.get_one::<String>("min-spread").unwrap().parse()?;
    let qty: i64 = matches.get_one::<String>("qty").unwrap().parse()?;
    let max_pos: i64 = matches.get_one::<String>("max-pos").unwrap().parse()?;
    let list_files = matches.get_flag("list-files");
    let list_markets = matches.get_flag("list-markets");
    let all_markets = matches.get_flag("all-markets");
    let summary_only = matches.get_flag("summary-only");
    let debug_log = matches.get_flag("debug-log");
    let pnl_log = matches.get_flag("pnl-log");

    // Verify data directory exists
    if !Path::new(data_dir).exists() {
        anyhow::bail!("Data directory not found: {}", data_dir);
    }

    // Find available data file pairs
    let file_pairs = find_data_file_pairs(data_dir, pattern)?;
    
    if file_pairs.is_empty() {
        anyhow::bail!("No matching orderbook/trades file pairs found in: {}", data_dir);
    }

    if list_files {
        println!("📁 Available data file pairs in {}:\n", data_dir);
        for (i, (orderbook, trades)) in file_pairs.iter().enumerate() {
            println!("  {}. Orderbook: {}", i + 1, orderbook.file_name().unwrap().to_string_lossy());
            println!("     Trades:    {}", trades.file_name().unwrap().to_string_lossy());
            println!();
        }
        return Ok(());
    }

    // Use the most recent file pair (or the one matching the pattern)
    let (orderbook_path, trades_path) = &file_pairs[0];
    
    // Get available markets from the data files
    let available_markets = get_available_markets(orderbook_path, trades_path)?;
    
    if list_markets {
        println!("📊 Available markets in data files:\n");
        for (i, market) in available_markets.iter().enumerate() {
            println!("  {}. {}", i + 1, market);
        }
        println!("\nOptions:");
        println!("  --market <TICKER>     : Backtest a specific market");
        println!("  --prefix <PREFIX>     : Backtest all markets with prefix");
        println!("  --all-markets         : Backtest all markets");
        return Ok(());
    }

    if available_markets.is_empty() {
        anyhow::bail!("No market tickers found in the data files");
    }

    println!("🎯 Running Kalshi Backtest");
    println!("📁 Data directory: {}", data_dir);
    println!("📊 Orderbook: {}", orderbook_path.file_name().unwrap().to_string_lossy());
    println!("💹 Trades: {}", trades_path.file_name().unwrap().to_string_lossy());
    println!("🕒 Latency: {}ms", latency_ms);
    println!("🎯 Strategy: {}", strategy_type);
    println!();

    let config = SimConfig {
        latency: Duration::from_millis(latency_ms),
        start_time: 0,
        end_time: None,
    };

    // Determine which markets to backtest
    let markets_to_test = if all_markets {
        available_markets
    } else if let Some(prefix) = market_prefix {
        let filtered_markets: Vec<String> = available_markets
            .into_iter()
            .filter(|market| market.starts_with(prefix))
            .collect();
        
        if filtered_markets.is_empty() {
            anyhow::bail!("No markets found with prefix '{}'. Use --list-markets to see available markets", prefix);
        }
        
        println!("🔍 Found {} markets with prefix '{}'", filtered_markets.len(), prefix);
        if !summary_only {
            for market in &filtered_markets {
                println!("  📈 {}", market);
            }
        }
        println!();
        
        filtered_markets
    } else if let Some(market) = target_market {
        if !available_markets.contains(market) {
            anyhow::bail!("Market '{}' not found in data files. Available markets: {:?}", market, available_markets);
        }
        vec![market.clone()]
    } else {
        // Default to the first market if none specified
        println!("💡 No specific market selected, using: {}", available_markets[0]);
        println!("   Use --list-markets to see all available markets");
        println!("   Use --prefix <PREFIX> to test multiple markets");
        println!();
        vec![available_markets[0].clone()]
    };

    // Run backtest for each selected market and collect results
    let mut market_results = Vec::new();
    
    for (i, market_ticker) in markets_to_test.iter().enumerate() {
        if markets_to_test.len() > 1 && !summary_only {
            println!("🏪 Market {}/{}: {}", i + 1, markets_to_test.len(), market_ticker);
            println!("{}", "=".repeat(60));
        }

        let result = run_backtest_for_market(
            orderbook_path,
            trades_path,
            market_ticker,
            &config,
            strategy_type,
            min_spread,
            qty,
            max_pos,
            debug_log,
            pnl_log,
        );

        match result {
            Ok(report) => {
                let perf = calculate_performance_metrics(&report);
                
                market_results.push(MarketResult {
                    market_ticker: market_ticker.clone(),
                    report: report.clone(),
                    performance: perf.clone(),
                });

                if !summary_only {
                    println!("✅ Backtest completed for {}!\n", market_ticker);
                    println!("{}", perf);
                    
                    if report.fills.len() > 0 {
                        if report.fills.len() <= 6 {
                            // Show all fills if 6 or fewer
                            println!("\n📋 All Fills:");
                            for fill in &report.fills {
                                println!("  {} contracts @ {} cents", fill.qty, fill.price);
                            }
                        } else {
                            // Show first 3 and last 3 fills
                            println!("\n📋 First & Last Fills:");
                            
                            println!("  First 3:");
                            for fill in report.fills.iter().take(3) {
                                println!("    {} contracts @ {} cents", fill.qty, fill.price);
                            }
                            
                            println!("  ... {} fills in between ...", report.fills.len() - 6);
                            
                            println!("  Last 3:");
                            for fill in report.fills.iter().rev().take(3).rev() {
                                println!("    {} contracts @ {} cents", fill.qty, fill.price);
                            }
                        }
                    }

                    println!("\n📊 Order Summary:");
                    println!("  Total Orders: {}", report.metrics.total_orders);
                    println!("  Filled Orders: {}", report.metrics.total_fills);
                    println!("  Cancelled Orders: {}", report.metrics.cancelled_orders);
                    println!("  Expired Orders: {}", report.metrics.expired_orders);
                } else {
                    print!(".");
                    if (i + 1) % 50 == 0 {
                        println!(" {}/{}", i + 1, markets_to_test.len());
                    }
                }
            }
            Err(e) => {
                if !summary_only {
                    eprintln!("❌ Backtest failed for {}: {}", market_ticker, e);
                } else {
                    print!("✗");
                }
            }
        }

        if i < markets_to_test.len() - 1 && !summary_only {
            println!("\n{}\n", "─".repeat(60));
        }
    }

    if summary_only && markets_to_test.len() > 1 {
        println!(); // New line after progress dots
    }

    // Display aggregated results if multiple markets were tested
    if markets_to_test.len() > 1 {
        println!("\n{}", "═".repeat(80));
        println!("📊 AGGREGATED RESULTS ACROSS {} MARKETS", markets_to_test.len());
        println!("{}", "═".repeat(80));
        
        let aggregated = calculate_aggregated_metrics(&market_results);
        display_aggregated_metrics(&aggregated);
        
        // Show top performing markets
        if !summary_only && market_results.len() > 3 {
            display_top_markets(&market_results);
        }
    }

    println!("\n🎉 Completed backtests for {} markets", markets_to_test.len());
    Ok(())
}

fn calculate_aggregated_metrics(results: &[MarketResult]) -> AggregatedMetrics {
    if results.is_empty() {
        return AggregatedMetrics {
            total_markets: 0,
            profitable_markets: 0,
            total_gross_pnl: 0.0,
            total_volume: 0,
            total_trades: 0,
            total_orders: 0,
            total_fills: 0,
            total_cancelled: 0,
            total_expired: 0,
            avg_win_rate: 0.0,
            best_market: None,
            worst_market: None,
            best_pnl: 0.0,
            worst_pnl: 0.0,
        };
    }

    let mut total_gross_pnl = 0.0;
    let mut total_volume = 0i64;
    let mut total_trades = 0usize;
    let mut total_orders = 0usize;
    let mut total_fills = 0usize;
    let mut total_cancelled = 0usize;
    let mut total_expired = 0usize;
    let mut total_win_rate = 0.0;
    let mut profitable_markets = 0usize;
    
    let mut best_pnl = f64::NEG_INFINITY;
    let mut worst_pnl = f64::INFINITY;
    let mut best_market = None;
    let mut worst_market = None;

    for result in results {
        let pnl = result.performance.gross_pnl_dollars;
        total_gross_pnl += pnl;
        total_volume += result.performance.total_volume;
        total_trades += result.performance.total_trades;
        total_orders += result.report.metrics.total_orders;
        total_fills += result.report.metrics.total_fills;
        total_cancelled += result.report.metrics.cancelled_orders;
        total_expired += result.report.metrics.expired_orders;
        total_win_rate += result.performance.win_rate;
        
        if pnl > 0.0 {
            profitable_markets += 1;
        }
        
        if pnl > best_pnl {
            best_pnl = pnl;
            best_market = Some(result.market_ticker.clone());
        }
        
        if pnl < worst_pnl {
            worst_pnl = pnl;
            worst_market = Some(result.market_ticker.clone());
        }
    }

    AggregatedMetrics {
        total_markets: results.len(),
        profitable_markets,
        total_gross_pnl,
        total_volume,
        total_trades,
        total_orders,
        total_fills,
        total_cancelled,
        total_expired,
        avg_win_rate: total_win_rate / results.len() as f64,
        best_market,
        worst_market,
        best_pnl,
        worst_pnl,
    }
}

fn display_aggregated_metrics(metrics: &AggregatedMetrics) {
    println!("💰 Total Gross PnL: ${:.2}", metrics.total_gross_pnl);
    println!("📈 Profitable Markets: {}/{} ({:.1}%)", 
             metrics.profitable_markets, 
             metrics.total_markets,
             (metrics.profitable_markets as f64 / metrics.total_markets as f64) * 100.0);
    
    println!("\n📊 Trading Volume:");
    println!("  Total Volume: {} contracts", metrics.total_volume);
    println!("  Total Trades: {}", metrics.total_trades);
    println!("  Avg PnL per Trade: ${:.4}", 
             if metrics.total_trades > 0 { metrics.total_gross_pnl / metrics.total_trades as f64 } else { 0.0 });
    
    println!("\n🎯 Order Statistics:");
    println!("  Total Orders: {}", metrics.total_orders);
    println!("  Filled Orders: {} ({:.1}%)", 
             metrics.total_fills,
             if metrics.total_orders > 0 { (metrics.total_fills as f64 / metrics.total_orders as f64) * 100.0 } else { 0.0 });
    println!("  Cancelled Orders: {} ({:.1}%)", 
             metrics.total_cancelled,
             if metrics.total_orders > 0 { (metrics.total_cancelled as f64 / metrics.total_orders as f64) * 100.0 } else { 0.0 });
    println!("  Expired Orders: {} ({:.1}%)", 
             metrics.total_expired,
             if metrics.total_orders > 0 { (metrics.total_expired as f64 / metrics.total_orders as f64) * 100.0 } else { 0.0 });
    
    println!("\n🏆 Performance Highlights:");
    println!("  Average Win Rate: {:.1}%", metrics.avg_win_rate * 100.0);
    
    if let Some(best) = &metrics.best_market {
        println!("  Best Market: {} (${:.2})", best, metrics.best_pnl);
    }
    
    if let Some(worst) = &metrics.worst_market {
        println!("  Worst Market: {} (${:.2})", worst, metrics.worst_pnl);
    }
}

fn display_top_markets(results: &[MarketResult]) {
    let mut sorted_results = results.to_vec();
    sorted_results.sort_by(|a, b| b.performance.gross_pnl_dollars.partial_cmp(&a.performance.gross_pnl_dollars).unwrap());
    
    println!("\n🏆 TOP 5 PERFORMING MARKETS:");
    for (i, result) in sorted_results.iter().take(5).enumerate() {
        println!("  {}. {} - ${:.2} ({} trades)", 
                 i + 1, 
                 result.market_ticker,
                 result.performance.gross_pnl_dollars,
                 result.performance.total_trades);
    }
    
    println!("\n📉 BOTTOM 5 PERFORMING MARKETS:");
    for (i, result) in sorted_results.iter().rev().take(5).enumerate() {
        println!("  {}. {} - ${:.2} ({} trades)", 
                 i + 1, 
                 result.market_ticker,
                 result.performance.gross_pnl_dollars,
                 result.performance.total_trades);
    }
}

fn run_backtest_for_market(
    orderbook_path: &PathBuf,
    trades_path: &PathBuf,
    market_ticker: &str,
    config: &SimConfig,
    strategy_type: &str,
    min_spread: u8,
    qty: i64,
    max_pos: i64,
    debug_log: bool,
    pnl_log: bool,
) -> Result<InkBack::kalshi_types::Report> {
    // Create filtered CSV files for this specific market
    let temp_orderbook = create_filtered_csv(orderbook_path, market_ticker, "orderbook")?;
    let temp_trades = create_filtered_csv(trades_path, market_ticker, "trades")?;

    // Choose between debug logging backtest and regular backtest
    let result = if debug_log {
        run_backtest_with_debug_logging(
            &temp_orderbook,
            &temp_trades,
            market_ticker,
            config,
            strategy_type,
            min_spread,
            qty,
            max_pos,
            debug_log,
            pnl_log,
        )
    } else {
        let backtest = KalshiBacktest::new(config.clone());
        
        // Create strategy based on user input
        let mut strategy: Box<dyn KalshiStrategy> = match strategy_type {
        "spread_mm" => {
            let params = SpreadMmParams {
                min_spread,
                quote_quantity: qty,
                max_position: max_pos,
                order_ttl: Duration::from_secs(5),
            };
            Box::new(SpreadMmStrategy::new(params))
        }
        "trailing_spread_mm" => {
            let params = TrailingMmParams {
                min_spread_for_trailing: min_spread.max(3), // Ensure minimum for trailing
                trail_distance: 2,
                max_position: max_pos,
                quote_quantity: qty,
                order_ttl: Duration::from_secs(3),
                min_move_for_replace: 1,
            };
            Box::new(TrailingMmStrategy::new(params))
        }
        "directional_yes" => {
            Box::new(DirectionalStrategy::new(Side::Yes, qty, 60))
        }
        "directional_no" => {
            Box::new(DirectionalStrategy::new(Side::No, qty, 40))
        }
        _ => {
            anyhow::bail!("Unknown strategy: {}. Use spread_mm, trailing_spread_mm, directional_yes, or directional_no", strategy_type);
        }
        };

        // Wrap strategy with PnL logging if enabled
        if pnl_log {
            let pnl_log_file = std::fs::File::create("kalshi_pnl.log")?;
            let pnl_writer = Rc::new(RefCell::new(pnl_log_file));
            
            // Write header to PnL log
            {
                let mut log = pnl_writer.borrow_mut();
                writeln!(log, "=== KALSHI PnL LOG ===")?;
                writeln!(log, "Market: {}", market_ticker)?;
                writeln!(log, "Strategy: {}", strategy_type)?;
                writeln!(log, "Step-by-step PnL calculations in table format...\n")?;
                writeln!(log, "Legend:")?;
                writeln!(log, "  TS = Timestamp")?;
                writeln!(log, "  SIDE = YESBUY/YESSELL/NOBUY/NOSELL")?;
                writeln!(log, "  PRICE = Fill price in cents")?;
                writeln!(log, "  QTY = Quantity filled")?;
                writeln!(log, "  POSITION = YES position before->after")?;
                writeln!(log, "  COST_BASIS = Average cost basis in cents")?;
                writeln!(log, "  REALIZED = Realized PnL in cents")?;
                writeln!(log, "  UNREALIZED = Unrealized PnL in cents")?;
                writeln!(log, "  TOTAL = Total PnL (realized + unrealized) in cents")?;
                writeln!(log, "  MAX_P/L = Max profit/Max loss in cents\n")?;
            }
            
            let mut pnl_logging_strategy = PnlLoggingStrategy::new(strategy, pnl_writer);
            let result = backtest.run_from_files(&temp_orderbook, &temp_trades, &mut pnl_logging_strategy)?;
            
            // Write settlement summary to PnL log
            let settlement_pnl = pnl_logging_strategy.calculate_final_settlement();
            if let Ok(mut log) = pnl_logging_strategy.pnl_writer.try_borrow_mut() {
                writeln!(log, "\n=== PnL SUMMARY ===")?;
                writeln!(log, "Final Position: {}", pnl_logging_strategy.position)?;
                writeln!(log, "Final Cost Basis: {:.2}¢", pnl_logging_strategy.avg_cost_basis)?;
                writeln!(log, "Realized PnL: {}¢", pnl_logging_strategy.realized_pnl)?;
                writeln!(log, "Settlement PnL: {}¢", settlement_pnl)?;
                writeln!(log, "Total Final PnL: {}¢", pnl_logging_strategy.realized_pnl + settlement_pnl)?;
                writeln!(log, "Max Profit: {}¢", pnl_logging_strategy.max_profit)?;
                writeln!(log, "Max Loss: {}¢", pnl_logging_strategy.max_loss)?;
                writeln!(log, "\nPnL log complete.")?;
            }
            
            Ok(result)
        } else {
            backtest.run_from_files(&temp_orderbook, &temp_trades, &mut *strategy)
        }
    };

    // Clean up temporary files
    let _ = fs::remove_file(&temp_orderbook);
    let _ = fs::remove_file(&temp_trades);

    result
}

fn create_filtered_csv(source_path: &PathBuf, market_ticker: &str, file_type: &str) -> Result<String> {
    let temp_filename = format!("temp_{}_{}.csv", file_type, market_ticker.replace("-", "_"));
    let mut reader = ReaderBuilder::new().has_headers(true).from_path(source_path)?;
    let mut writer = csv::Writer::from_path(&temp_filename)?;

    // Copy headers
    let headers = reader.headers()?.clone();
    writer.write_record(&headers)?;

    // Find the market_ticker column index
    let ticker_col_index = headers.iter().position(|h| h == "market_ticker" || h == "ticker")
        .ok_or_else(|| anyhow::anyhow!("No market_ticker or ticker column found"))?;

    // Copy only rows for the specified market
    for result in reader.records() {
        let record = result?;
        if let Some(ticker) = record.get(ticker_col_index) {
            if ticker == market_ticker {
                writer.write_record(&record)?;
            }
        }
    }

    writer.flush()?;
    Ok(temp_filename)
}

fn get_available_markets(orderbook_path: &PathBuf, trades_path: &PathBuf) -> Result<Vec<String>> {
    let mut markets = HashSet::new();

    // Check orderbook file for markets
    let mut reader = ReaderBuilder::new().has_headers(true).from_path(orderbook_path)?;
    let headers = reader.headers()?;
    
    if let Some(ticker_col_index) = headers.iter().position(|h| h == "market_ticker" || h == "ticker") {
        for result in reader.records().take(1000) { // Sample first 1000 rows for performance
            let record = result?;
            if let Some(ticker) = record.get(ticker_col_index) {
                markets.insert(ticker.to_string());
            }
        }
    }

    // Also check trades file for any additional markets
    let mut reader = ReaderBuilder::new().has_headers(true).from_path(trades_path)?;
    let headers = reader.headers()?;
    
    if let Some(ticker_col_index) = headers.iter().position(|h| h == "market_ticker" || h == "ticker") {
        for result in reader.records().take(1000) { // Sample first 1000 rows for performance
            let record = result?;
            if let Some(ticker) = record.get(ticker_col_index) {
                markets.insert(ticker.to_string());
            }
        }
    }

    let mut market_list: Vec<String> = markets.into_iter().collect();
    market_list.sort();
    Ok(market_list)
}

/// Find matching orderbook and trades CSV file pairs in the given directory
fn find_data_file_pairs(data_dir: &str, pattern_filter: Option<&String>) -> Result<Vec<(PathBuf, PathBuf)>> {
    let dir_path = Path::new(data_dir);
    let entries = fs::read_dir(dir_path)?;

    let mut orderbook_files = Vec::new();
    let mut trades_files = Vec::new();

    // Collect all orderbook and trades files
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if filename.starts_with("orderbook") && filename.ends_with(".csv") {
                orderbook_files.push(path);
            } else if filename.starts_with("trades") && filename.ends_with(".csv") {
                trades_files.push(path);
            }
        }
    }

    // Sort files by name (which includes timestamp) - newest first
    orderbook_files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    trades_files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    let mut file_pairs = Vec::new();

    // Try to match orderbook and trades files by timestamp pattern
    for orderbook_file in &orderbook_files {
        if let Some(ob_filename) = orderbook_file.file_name().and_then(|n| n.to_str()) {
            // Extract timestamp pattern from orderbook filename
            if let Some(timestamp) = extract_timestamp_pattern(ob_filename) {
                // Apply pattern filter if specified
                if let Some(filter) = pattern_filter {
                    if !timestamp.contains(filter) {
                        continue;
                    }
                }
                
                // Look for matching trades file
                for trades_file in &trades_files {
                    if let Some(trades_filename) = trades_file.file_name().and_then(|n| n.to_str()) {
                        if trades_filename.contains(&timestamp) {
                            file_pairs.push((orderbook_file.clone(), trades_file.clone()));
                            break;
                        }
                    }
                }
            }
        }
    }

    Ok(file_pairs)
}

/// Extract timestamp pattern from filename (e.g., "20250723_135706" from "orderbook_20250723_135706.csv")
fn extract_timestamp_pattern(filename: &str) -> Option<String> {
    // Look for pattern like YYYYMMDD_HHMMSS
    let parts: Vec<&str> = filename.split('_').collect();
    if parts.len() >= 3 {
        // Assume timestamp is the last two parts before .csv
        let timestamp_parts = &parts[parts.len()-2..];
        Some(timestamp_parts.join("_").replace(".csv", ""))
    } else {
        None
    }
} 

/// Run backtest with debug logging for chronological event analysis
fn run_backtest_with_debug_logging(
    orderbook_path: &str,
    trades_path: &str,
    market_ticker: &str,
    config: &SimConfig,
    strategy_type: &str,
    min_spread: u8,
    qty: i64,
    max_pos: i64,
    debug_log: bool,
    pnl_log: bool,
) -> Result<Report> {
    println!("📝 Debug logging enabled - writing to kalshi_debug.log");
    
    // Create strategy based on user input
    let strategy: Box<dyn KalshiStrategy> = match strategy_type {
        "spread_mm" => {
            let params = SpreadMmParams {
                min_spread,
                quote_quantity: qty,
                max_position: max_pos,
                order_ttl: Duration::from_secs(5),
            };
            Box::new(SpreadMmStrategy::new(params))
        }
        "trailing_spread_mm" => {
            let params = TrailingMmParams {
                min_spread_for_trailing: min_spread.max(3),
                trail_distance: 2,
                max_position: max_pos,
                quote_quantity: qty,
                order_ttl: Duration::from_secs(3),
                min_move_for_replace: 1,
            };
            Box::new(TrailingMmStrategy::new(params))
        }
        "directional_yes" => {
            Box::new(DirectionalStrategy::new(Side::Yes, qty, 60))
        }
        "directional_no" => {
            Box::new(DirectionalStrategy::new(Side::No, qty, 40))
        }
        _ => {
            anyhow::bail!("Unknown strategy: {}. Use spread_mm, trailing_spread_mm, directional_yes, or directional_no", strategy_type);
        }
    };

    // Create the event stream
    let orderbook_iter = parse_orderbook(orderbook_path)?;
    let trades_iter = parse_trades(trades_path)?;
    let event_stream = merge(orderbook_iter, trades_iter);
    
    // Create a shared log file
    let log_file = std::fs::File::create("kalshi_debug.log")?;
    let log_writer = Rc::new(RefCell::new(log_file));
    
    // Write header to log
    {
        let mut log = log_writer.borrow_mut();
        writeln!(log, "=== KALSHI BACKTEST DEBUG LOG ===")?;
        writeln!(log, "Market: {}", market_ticker)?;
        writeln!(log, "Strategy: {}", strategy_type)?;
        writeln!(log, "Events logged in strict chronological order by timestamp...\n")?;
    }
    
    let log_writer_clone = log_writer.clone();
    
    // Create logging wrapper for events
    let logged_events = event_stream.map(move |event_result| {
        event_result.map(|event| {
            // Log all events as they're processed in chronological order
            if let Ok(mut log) = log_writer_clone.try_borrow_mut() {
                match &event {
                    Event::Snapshot { seq, ts, ticker, yes_levels, no_levels } => {
                        let _ = writeln!(log, "SNAPSHOT: ts={}, seq={}, ticker={}, yes_levels={:?}, no_levels={:?}",
                            ts, seq, ticker, yes_levels, no_levels);
                    }
                    Event::Delta { seq, ts, ticker, side, price, delta } => {
                        let _ = writeln!(log, "DELTA: ts={}, seq={}, ticker={}, side={:?}, price={}¢, delta={}",
                            ts, seq, ticker, side, price, delta);
                    }
                    Event::Trade { seq, ts, ticker, yes_price, no_price, qty, taker } => {
                        let _ = writeln!(log, "TRADE: ts={}, seq={}, ticker={}, yes_price={}¢, no_price={}¢, qty={}, taker={:?}",
                            ts, seq, ticker, yes_price, no_price, qty, taker);
                    }
                }
                let _ = log.flush();
            }
            event
        })
    });
    
    // Create logging wrapper for strategy
    let mut logging_strategy: Box<dyn KalshiStrategy> = if pnl_log {
        // PnL logging enabled (debug logging is always enabled in this function)
        let pnl_log_file = std::fs::File::create("kalshi_pnl.log")?;
        let pnl_writer = Rc::new(RefCell::new(pnl_log_file));
        
        // Write header to PnL log
        {
            let mut log = pnl_writer.borrow_mut();
            writeln!(log, "=== KALSHI PnL LOG ===")?;
            writeln!(log, "Market: {}", market_ticker)?;
            writeln!(log, "Strategy: {}", strategy_type)?;
            writeln!(log, "Step-by-step PnL calculations in table format...\n")?;
            writeln!(log, "Legend:")?;
            writeln!(log, "  TS = Timestamp")?;
            writeln!(log, "  SIDE = BUY/SELL")?;
            writeln!(log, "  PRICE = Fill price in cents")?;
            writeln!(log, "  QTY = Quantity filled")?;
            writeln!(log, "  POSITION = Position before->after")?;
            writeln!(log, "  COST_BASIS = Average cost basis in cents")?;
            writeln!(log, "  REALIZED = Realized PnL in cents")?;
            writeln!(log, "  UNREALIZED = Unrealized PnL in cents")?;
            writeln!(log, "  TOTAL = Total PnL (realized + unrealized) in cents")?;
            writeln!(log, "  MAX_P/L = Max profit/Max loss in cents\n")?;
        }
        
        // Create PnL logging wrapper around the debug logging wrapper
        let debug_wrapper = LoggingStrategy::new(strategy, log_writer);
        Box::new(PnlLoggingStrategy::new(Box::new(debug_wrapper), pnl_writer))
    } else {
        // Only debug logging enabled (no PnL logging)
        Box::new(LoggingStrategy::new(strategy, log_writer))
    };
    
    // Run the backtest
    let result = run_backtest(config.clone(), &mut *logging_strategy, logged_events)?;
    
    // Write summary to log
    if pnl_log {
        // Both PnL and debug logging enabled
        if let Some(pnl_wrapper) = logging_strategy.as_any().downcast_mut::<PnlLoggingStrategy>() {
            // Calculate final settlement PnL
            let settlement_pnl = pnl_wrapper.calculate_final_settlement();
            
            // Write PnL summary
            if let Ok(mut log) = pnl_wrapper.pnl_writer.try_borrow_mut() {
                writeln!(log, "\n=== PnL SUMMARY ===")?;
                writeln!(log, "Final Position: {}", pnl_wrapper.position)?;
                writeln!(log, "Final Cost Basis: {:.2}¢", pnl_wrapper.avg_cost_basis)?;
                writeln!(log, "Realized PnL: {}¢", pnl_wrapper.realized_pnl)?;
                writeln!(log, "Settlement PnL: {}¢", settlement_pnl)?;
                writeln!(log, "Total Final PnL: {}¢", pnl_wrapper.realized_pnl + settlement_pnl)?;
                writeln!(log, "Max Profit: {}¢", pnl_wrapper.max_profit)?;
                writeln!(log, "Max Loss: {}¢", pnl_wrapper.max_loss)?;
                writeln!(log, "\nPnL log complete.")?;
            }
            
            // Write debug summary to the inner debug wrapper
            if let Some(debug_strategy) = pnl_wrapper.inner.as_any().downcast_mut::<LoggingStrategy>() {
                let mut log = debug_strategy.log_writer.borrow_mut();
                writeln!(log, "\n=== BACKTEST SUMMARY ===")?;
                writeln!(log, "Total orders: {}", result.metrics.total_orders)?;
                writeln!(log, "Total fills: {}", result.metrics.total_fills)?;
                writeln!(log, "Cancelled orders: {}", result.metrics.cancelled_orders)?;
                writeln!(log, "Expired orders: {}", result.metrics.expired_orders)?;
                writeln!(log, "\nDebug log complete.")?;
            }
        }
    } else {
        // Only debug logging enabled (no PnL logging)
        if let Some(debug_strategy) = logging_strategy.as_any().downcast_mut::<LoggingStrategy>() {
            let mut log = debug_strategy.log_writer.borrow_mut();
            writeln!(log, "\n=== BACKTEST SUMMARY ===")?;
            writeln!(log, "Total orders: {}", result.metrics.total_orders)?;
            writeln!(log, "Total fills: {}", result.metrics.total_fills)?;
            writeln!(log, "Cancelled orders: {}", result.metrics.cancelled_orders)?;
            writeln!(log, "Expired orders: {}", result.metrics.expired_orders)?;
            writeln!(log, "\nDebug log complete.")?;
        }
    }
    
    Ok(result)
}

/// Wrapper strategy that logs all actions in chronological order
struct LoggingStrategy {
    inner: Box<dyn KalshiStrategy>,
    log_writer: Rc<RefCell<std::fs::File>>,
}

impl LoggingStrategy {
    fn new(strategy: Box<dyn KalshiStrategy>, log_writer: Rc<RefCell<std::fs::File>>) -> Self {
        Self {
            inner: strategy,
            log_writer,
        }
    }
}

impl KalshiStrategy for LoggingStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        // Log orderbook update
        if let Ok(mut log) = self.log_writer.try_borrow_mut() {
            let _ = writeln!(log, "BOOK_UPDATE: ts={}, ticker={}, bid={}¢, ask={}¢, spread={}¢",
                md.ts, md.ticker, md.bid, md.ask, md.ask.saturating_sub(md.bid));
            let _ = log.flush();
        }
        
        // Get strategy instructions
        let instructions = self.inner.on_book(md);
        
        // Log strategy actions
        for instr in &instructions {
            if let Ok(mut log) = self.log_writer.try_borrow_mut() {
                match instr {
                    OrderInstr::Limit { side, price, qty, .. } => {
                        let _ = writeln!(log, "STRATEGY_ORDER: ts={}, side={:?}, price={}¢, qty={}",
                            md.ts, side, price, qty);
                    }
                    OrderInstr::Cancel { id } => {
                        let _ = writeln!(log, "STRATEGY_CANCEL: ts={}, order_id={}", md.ts, id);
                    }
                }
                            let _ = log.flush();
        }
    }

        instructions
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Log fill
        if let Ok(mut log) = self.log_writer.try_borrow_mut() {
            let _ = writeln!(log, "FILL: ts={}, order_id={}, price={}¢, qty={}",
                fill.ts, fill.id, fill.price, fill.qty);
            let _ = log.flush();
        }
        
        // Forward to inner strategy
        self.inner.on_fill(fill);
    }

    fn on_orders_created(&mut self, order_mappings: Vec<(usize, OrderId)>) {
        // Log order creations
        if let Ok(mut log) = self.log_writer.try_borrow_mut() {
            for (instruction_index, order_id) in &order_mappings {
                let _ = writeln!(log, "ORDER_CREATED: instruction_index={}, assigned_order_id={}",
                    instruction_index, order_id);
            }
            let _ = log.flush();
        }
        
        // Forward to inner strategy
        self.inner.on_orders_created(order_mappings);
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self.inner.as_any()
    }
} 

/// Wrapper strategy that logs PnL calculations step by step
struct PnlLoggingStrategy {
    inner: Box<dyn KalshiStrategy>,
    pnl_writer: Rc<RefCell<std::fs::File>>,
    // Track position and cost basis for PnL calculations
    position: i64,
    total_cost: i64,
    avg_cost_basis: f64,
    realized_pnl: i64,
    max_profit: i64,
    max_loss: i64,
}

impl PnlLoggingStrategy {
    fn new(strategy: Box<dyn KalshiStrategy>, pnl_writer: Rc<RefCell<std::fs::File>>) -> Self {
        Self {
            inner: strategy,
            pnl_writer,
            position: 0,
            total_cost: 0,
            avg_cost_basis: 0.0,
            realized_pnl: 0,
            max_profit: 0,
            max_loss: 0,
        }
    }

    fn log_pnl_update(&mut self, fill: &Fill, position_delta: i64, is_position_increasing: bool, is_yes_order: bool) {
        if let Ok(mut log) = self.pnl_writer.try_borrow_mut() {
            let old_position = self.position;
            let old_cost_basis = self.avg_cost_basis;
            let old_realized_pnl = self.realized_pnl;
            
            // Calculate new position
            let new_position = self.position + position_delta;
            
            // Calculate PnL impact
            let pnl_impact = if !is_position_increasing && self.position != 0 {
                // This is a round-trip trade (reducing position)
                let round_trip_pnl = if self.position > 0 {
                    // Long position being reduced: PnL = (exit_price - cost_basis) * qty
                    (fill.price as i64 - self.avg_cost_basis as i64) * position_delta.abs()
                } else {
                    // Short position being reduced: PnL = (cost_basis - exit_price) * qty
                    (self.avg_cost_basis as i64 - fill.price as i64) * position_delta.abs()
                };
                round_trip_pnl
            } else {
                0
            };
            
            let new_realized_pnl = self.realized_pnl + pnl_impact;
            
            // Calculate unrealized PnL at current market price (using fill price as proxy)
            let unrealized_pnl = if new_position != 0 {
                if new_position > 0 {
                    // Long position: unrealized = (current_price - cost_basis) * position
                    (fill.price as i64 - self.avg_cost_basis as i64) * new_position
                } else {
                    // Short position: unrealized = (cost_basis - current_price) * abs(position)
                    (self.avg_cost_basis as i64 - fill.price as i64) * new_position.abs()
                }
            } else {
                0
            };
            
            let total_pnl = new_realized_pnl + unrealized_pnl;
            
            // Update max profit/loss
            if total_pnl > self.max_profit {
                self.max_profit = total_pnl;
            }
            if total_pnl < self.max_loss {
                self.max_loss = total_pnl;
            }
            
            // Log the PnL calculation in table format
            let side = if is_yes_order { "YES" } else { "NO" };
            let action = if position_delta > 0 { "BUY" } else { "SELL" };
            let trade_type = if !is_position_increasing && self.position != 0 { "ROUND-TRIP" } else { "POSITION" };
            
            // Print table header only once (on first fill)
            if self.position == 0 && position_delta > 0 {
                let _ = writeln!(log, "{:>8} | {:>6} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8}",
                    "TS", "SIDE", "PRICE", "QTY", "POSITION", "COST_BASIS", "REALIZED", "UNREALIZED", "TOTAL", "MAX_P/L");
                let _ = writeln!(log, "{:-<8}-+-{:-<6}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}",
                    "", "", "", "", "", "", "", "", "", "");
            }
            
            let _ = writeln!(log, "{:>8} | {:>6} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8}",
                fill.ts, format!("{}{}", side, action), format!("{}¢", fill.price), fill.qty, 
                format!("{}->{}", old_position, new_position),
                format!("{:.1}¢", self.avg_cost_basis),
                format!("{}¢", new_realized_pnl),
                format!("{}¢", unrealized_pnl),
                format!("{}¢", total_pnl),
                format!("{}/{}", self.max_profit, self.max_loss));
            let _ = log.flush();
        }
    }

    fn calculate_final_settlement(&self) -> i64 {
        if self.position == 0 {
            return 0;
        }
        
        // Determine market resolution based on final orderbook state
        // For this example, we'll assume YES resolution (98¢) based on the debug log
        // In practice, this would come from the actual market resolution
        let market_resolved_to_yes = true; // Assume YES resolution for now
        
        let settlement_pnl = if self.position > 0 {
            // We have a long YES position
            if market_resolved_to_yes {
                // YES wins: we get 100¢ per contract, minus what we paid
                (100 - self.avg_cost_basis as i64) * self.position
            } else {
                // NO wins: we get 0¢ per contract, minus what we paid
                (0 - self.avg_cost_basis as i64) * self.position
            }
        } else {
            // We have a short YES position (equivalent to long NO position)
            if market_resolved_to_yes {
                // YES wins: we owe 100¢ per contract, plus what we received
                (self.avg_cost_basis as i64 - 100) * self.position.abs()
            } else {
                // NO wins: we owe 0¢ per contract, plus what we received
                (self.avg_cost_basis as i64 - 0) * self.position.abs()
            }
        };
        
        settlement_pnl
    }
}

impl KalshiStrategy for PnlLoggingStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        // Forward to inner strategy
        self.inner.on_book(md)
    }

    fn on_fill(&mut self, fill: &Fill) {
        // We need to determine if this fill was a YES or NO order
        // Since we don't have direct access to order side info, we'll use a heuristic
        // but this should ideally come from the strategy's order tracking
        
        // For now, let's assume:
        // - If price < 50¢, this was likely a YES order (buying YES)
        // - If price >= 50¢, this was likely a NO order (buying NO)
        let is_yes_order = fill.price < 50;
        
        // Calculate position change:
        // - YES order: increases our YES position
        // - NO order: decreases our YES position (equivalent to selling YES)
        let position_delta = if is_yes_order {
            fill.qty  // Buying YES increases YES position
        } else {
            -fill.qty // Buying NO decreases YES position (selling YES)
        };
        
        let is_position_increasing = (self.position * position_delta) >= 0;
        
        // Log PnL calculation before updating position
        self.log_pnl_update(fill, position_delta, is_position_increasing, is_yes_order);
        
        // Update position tracking
        if is_position_increasing {
            // Position is growing, update cost basis
            let new_cost = fill.price as i64 * fill.qty.abs();
            self.total_cost += new_cost;
            self.position += position_delta;
            
            if self.position != 0 {
                self.avg_cost_basis = self.total_cost as f64 / self.position.abs() as f64;
            }
        } else {
            // Position is reducing (round-trip)
            let round_trip_pnl = if self.position > 0 {
                // We had a long YES position, now selling YES
                (fill.price as i64 - self.avg_cost_basis as i64) * fill.qty.abs()
            } else {
                // We had a short YES position, now buying YES
                (self.avg_cost_basis as i64 - fill.price as i64) * fill.qty.abs()
            };
            
            self.realized_pnl += round_trip_pnl;
            
            // Update cost basis by removing the portion we just closed
            let closed_qty = fill.qty.abs();
            self.total_cost -= self.avg_cost_basis as i64 * closed_qty;
            self.position += position_delta;
            
            // Recalculate cost basis if we still have position
            if self.position != 0 && self.total_cost > 0 {
                self.avg_cost_basis = self.total_cost as f64 / self.position.abs() as f64;
            } else if self.position == 0 {
                self.avg_cost_basis = 0.0;
                self.total_cost = 0;
            }
        }
        
        // Forward to inner strategy
        self.inner.on_fill(fill);
    }

    fn on_orders_created(&mut self, order_mappings: Vec<(usize, OrderId)>) {
        // Forward to inner strategy
        self.inner.on_orders_created(order_mappings);
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self.inner.as_any()
    }
} 