use clap::{Arg, Command};
use anyhow::Result;
use std::time::Duration;
use std::path::Path;

use InkBack::{KalshiBacktest, calculate_performance_metrics, SpreadMmStrategy, SpreadMmParams, DirectionalStrategy, SimConfig, Side};

fn main() -> Result<()> {
    let matches = Command::new("kalshi_backtest")
        .about("Run Kalshi backtests with real CSV data")
        .arg(
            Arg::new("orderbook")
                .long("orderbook")
                .short('o')
                .value_name("FILE")
                .help("Path to orderbook CSV file")
                .required(true),
        )
        .arg(
            Arg::new("trades")
                .long("trades")
                .short('t')
                .value_name("FILE")
                .help("Path to trades CSV file")
                .required(true),
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
        .get_matches();

    let orderbook_path = matches.get_one::<String>("orderbook").unwrap();
    let trades_path = matches.get_one::<String>("trades").unwrap();
    let strategy_type = matches.get_one::<String>("strategy").unwrap();
    let latency_ms: u64 = matches.get_one::<String>("latency").unwrap().parse()?;
    let min_spread: u8 = matches.get_one::<String>("min_spread").unwrap().parse()?;
    let qty: i64 = matches.get_one::<String>("qty").unwrap().parse()?;
    let max_pos: i64 = matches.get_one::<String>("max_pos").unwrap().parse()?;

    // Verify files exist
    if !Path::new(orderbook_path).exists() {
        anyhow::bail!("Orderbook file not found: {}", orderbook_path);
    }
    if !Path::new(trades_path).exists() {
        anyhow::bail!("Trades file not found: {}", trades_path);
    }

    println!("🎯 Running Kalshi Backtest");
    println!("📊 Orderbook: {}", orderbook_path);
    println!("💹 Trades: {}", trades_path);
    println!("🕒 Latency: {}ms", latency_ms);
    println!("🎯 Strategy: {}", strategy_type);
    println!();

    let config = SimConfig {
        latency: Duration::from_millis(latency_ms),
        start_time: 0,
        end_time: None,
    };

    let backtest = KalshiBacktest::new(config);

    // Create strategy based on user input
    let result = match strategy_type.as_str() {
        "spread_mm" => {
            println!("📈 Running Spread Market Making Strategy");
            let params = SpreadMmParams {
                min_spread,
                post_qty: qty,
                max_pos,
                order_ttl: Duration::from_secs(10),
            };
            let mut strategy = SpreadMmStrategy::new(params);
            backtest.run_from_files(orderbook_path, trades_path, &mut strategy)
        }
        "directional_yes" => {
            println!("📈 Running Directional Strategy (YES bias)");
            let mut strategy = DirectionalStrategy::new(Side::Yes, qty, 60);
            backtest.run_from_files(orderbook_path, trades_path, &mut strategy)
        }
        "directional_no" => {
            println!("📉 Running Directional Strategy (NO bias)");
            let mut strategy = DirectionalStrategy::new(Side::No, qty, 40);
            backtest.run_from_files(orderbook_path, trades_path, &mut strategy)
        }
        _ => {
            anyhow::bail!("Unknown strategy: {}. Use spread_mm, directional_yes, or directional_no", strategy_type);
        }
    };

    match result {
        Ok(report) => {
            let perf = calculate_performance_metrics(&report);
            println!("✅ Backtest completed successfully!\n");
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
            eprintln!("❌ Backtest failed: {}", e);
            eprintln!("\n💡 Common issues:");
            eprintln!("  - Check CSV file format");
            eprintln!("  - Ensure files contain valid data");
            eprintln!("  - Try a different strategy");
            std::process::exit(1);
        }
    }

    Ok(())
} 