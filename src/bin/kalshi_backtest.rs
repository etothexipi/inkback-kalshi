use clap::{Arg, Command};
use anyhow::Result;
use std::time::Duration;
use std::path::{Path, PathBuf};
use std::fs;
use std::collections::HashSet;
use csv::ReaderBuilder;

use InkBack::{KalshiBacktest, calculate_performance_metrics, SpreadMmStrategy, SpreadMmParams, DirectionalStrategy, SimConfig, Side};

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
            Arg::new("strategy")
                .long("strategy")
                .short('s')
                .value_name("STRATEGY")
                .help("Strategy type: spread_mm, directional_yes, directional_no")
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
            Arg::new("min_spread")
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
            Arg::new("max_pos")
                .long("max-pos")
                .value_name("SIZE")
                .help("Maximum position size")
                .default_value("200"),
        )
        .arg(
            Arg::new("list_files")
                .long("list-files")
                .help("List available data file pairs and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("list_markets")
                .long("list-markets")
                .help("List available market tickers in the data files and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("all_markets")
                .long("all-markets")
                .help("Run backtest on all markets found in the data files")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let data_dir = matches.get_one::<String>("data").unwrap();
    let pattern = matches.get_one::<String>("pattern");
    let target_market = matches.get_one::<String>("market");
    let strategy_type = matches.get_one::<String>("strategy").unwrap();
    let latency_ms: u64 = matches.get_one::<String>("latency").unwrap().parse()?;
    let min_spread: u8 = matches.get_one::<String>("min_spread").unwrap().parse()?;
    let qty: i64 = matches.get_one::<String>("qty").unwrap().parse()?;
    let max_pos: i64 = matches.get_one::<String>("max_pos").unwrap().parse()?;
    let list_files = matches.get_flag("list_files");
    let list_markets = matches.get_flag("list_markets");
    let all_markets = matches.get_flag("all_markets");

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
        println!("\nUse --market <TICKER> to backtest a specific market");
        println!("Use --all-markets to backtest all markets");
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
    } else if let Some(market) = target_market {
        if !available_markets.contains(market) {
            anyhow::bail!("Market '{}' not found in data files. Available markets: {:?}", market, available_markets);
        }
        vec![market.clone()]
    } else {
        // Default to the first market if none specified
        println!("💡 No specific market selected, using: {}", available_markets[0]);
        println!("   Use --list-markets to see all available markets");
        println!();
        vec![available_markets[0].clone()]
    };

    // Run backtest for each selected market
    for (i, market_ticker) in markets_to_test.iter().enumerate() {
        if markets_to_test.len() > 1 {
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
        );

        match result {
            Ok(report) => {
                let perf = calculate_performance_metrics(&report);
                println!("✅ Backtest completed for {}!\n", market_ticker);
                println!("{}", perf);
                
                if report.fills.len() > 0 {
                    println!("\n📋 Recent Fills:");
                    for fill in report.fills.iter().take(5) {
                        println!("  {} contracts @ {} cents", fill.qty, fill.price);
                    }
                    if report.fills.len() > 5 {
                        println!("  ... and {} more fills", report.fills.len() - 5);
                    }
                }

                println!("\n📊 Order Summary:");
                println!("  Total Orders: {}", report.metrics.total_orders);
                println!("  Filled Orders: {}", report.metrics.total_fills);
                println!("  Cancelled Orders: {}", report.metrics.cancelled_orders);
                println!("  Expired Orders: {}", report.metrics.expired_orders);
            }
            Err(e) => {
                eprintln!("❌ Backtest failed for {}: {}", market_ticker, e);
            }
        }

        if i < markets_to_test.len() - 1 {
            println!("\n{}\n", "─".repeat(60));
        }
    }

    if markets_to_test.len() > 1 {
        println!("\n🎉 Completed backtests for {} markets", markets_to_test.len());
    }

    Ok(())
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
) -> Result<InkBack::kalshi_types::Report> {
    // Create filtered CSV files for this specific market
    let temp_orderbook = create_filtered_csv(orderbook_path, market_ticker, "orderbook")?;
    let temp_trades = create_filtered_csv(trades_path, market_ticker, "trades")?;

    let backtest = KalshiBacktest::new(config.clone());

    // Create strategy based on user input
    let result = match strategy_type {
        "spread_mm" => {
            let params = SpreadMmParams {
                min_spread,
                post_qty: qty,
                max_pos,
                order_ttl: Duration::from_secs(10),
            };
            let mut strategy = SpreadMmStrategy::new(params);
            backtest.run_from_files(&temp_orderbook, &temp_trades, &mut strategy)
        }
        "directional_yes" => {
            let mut strategy = DirectionalStrategy::new(Side::Yes, qty, 60);
            backtest.run_from_files(&temp_orderbook, &temp_trades, &mut strategy)
        }
        "directional_no" => {
            let mut strategy = DirectionalStrategy::new(Side::No, qty, 40);
            backtest.run_from_files(&temp_orderbook, &temp_trades, &mut strategy)
        }
        _ => {
            anyhow::bail!("Unknown strategy: {}. Use spread_mm, directional_yes, or directional_no", strategy_type);
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