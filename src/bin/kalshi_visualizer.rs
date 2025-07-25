use clap::{Arg, Command};
use anyhow::Result;
use std::time::Duration;
use std::path::{Path, PathBuf};
use InkBack::{
    SpreadMmStrategy, SpreadMmParams, DirectionalStrategy, TrailingMmStrategy, TrailingMmParams,
    SimConfig, Side, kalshi_types::*, kalshi_strategy::KalshiStrategy,
    kalshi_csv_io::*, kalshi_event_stream::*, kalshi_exec_sim::run_backtest
};
use std::fs;
use std::io::Write;


use InkBack::kalshi_viz::{VisualizationData, OrderbookSnapshot, StrategyAction};

fn main() -> Result<()> {
    let matches = Command::new("kalshi_visualizer")
        .about("Visualize Kalshi orderbook dynamics and strategy behavior")
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
                .help("File pattern to match (e.g., '20250723' to match specific date)")
                .required(false),
        )
        .arg(
            Arg::new("market")
                .long("market")
                .short('m')
                .value_name("TICKER")
                .help("Market ticker to visualize")
                .required(true),
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
            Arg::new("time-window")
                .long("time-window")
                .value_name("SECONDS")
                .help("Time window to visualize around first and last fills (0 = full session)")
                .default_value("300"),
        )
        .get_matches();

    let data_dir = matches.get_one::<String>("data").unwrap();
    let pattern = matches.get_one::<String>("pattern");
    let market_ticker = matches.get_one::<String>("market").unwrap();
    let strategy_type = matches.get_one::<String>("strategy").unwrap();
    let latency_ms: u64 = matches.get_one::<String>("latency").unwrap().parse()?;
    let min_spread: u8 = matches.get_one::<String>("min-spread").unwrap().parse()?;
    let qty: i64 = matches.get_one::<String>("qty").unwrap().parse()?;
    let max_pos: i64 = matches.get_one::<String>("max-pos").unwrap().parse()?;
    let time_window: u64 = matches.get_one::<String>("time-window").unwrap().parse()?;

    // Verify data directory exists
    if !Path::new(data_dir).exists() {
        anyhow::bail!("Data directory not found: {}", data_dir);
    }

    // Find available data file pairs
    let file_pairs = find_data_file_pairs(data_dir, pattern)?;
    
    if file_pairs.is_empty() {
        anyhow::bail!("No matching orderbook/trades file pairs found in: {}", data_dir);
    }

    // Use the most recent file pair (or the one matching the pattern)
    let (orderbook_path, trades_path) = &file_pairs[0];

    println!("🎯 Kalshi Orderbook Visualizer");
    println!("📁 Data directory: {}", data_dir);
    println!("📊 Market: {}", market_ticker);
    println!("🎯 Strategy: {}", strategy_type);
    println!("🕒 Latency: {}ms", latency_ms);
    println!("📈 Orderbook: {}", orderbook_path.file_name().unwrap().to_string_lossy());
    println!("💹 Trades: {}", trades_path.file_name().unwrap().to_string_lossy());
    println!();

    let config = SimConfig {
        latency: Duration::from_millis(latency_ms),
        start_time: 0,
        end_time: None,
    };

    // Run the backtest and collect visualization data
    let viz_data = run_visualization_backtest(
        orderbook_path,
        trades_path,
        market_ticker,
        &config,
        strategy_type,
        min_spread,
        qty,
        max_pos,
        time_window,
    )?;

    println!("📊 Collected {} orderbook snapshots", viz_data.orderbook_snapshots.len());
    println!("🎯 Collected {} strategy actions", viz_data.strategy_actions.len());
    println!("💹 Collected {} fills", viz_data.fills.len());
    
    if viz_data.fills.is_empty() {
        println!("⚠️  No fills occurred - showing full session orderbook dynamics");
    } else {
        println!("📈 Showing orderbook dynamics around fills");
        let first_fill_time = viz_data.fills.first().unwrap().ts;
        let last_fill_time = viz_data.fills.last().unwrap().ts;
        println!("   First fill: {} ({})", first_fill_time, format_timestamp(first_fill_time));
        println!("   Last fill:  {} ({})", last_fill_time, format_timestamp(last_fill_time));
    }

    // Launch the visualization
    println!("\n🚀 Launching orderbook visualizer...");
    InkBack::kalshi_viz::run_visualizer(viz_data)?;

    Ok(())
}

fn run_visualization_backtest(
    orderbook_path: &PathBuf,
    trades_path: &PathBuf,
    market_ticker: &str,
    config: &SimConfig,
    strategy_type: &str,
    min_spread: u8,
    qty: i64,
    max_pos: i64,
    time_window: u64,
) -> Result<VisualizationData> {
    // Create filtered CSV files for this specific market
    let temp_orderbook = create_filtered_csv(orderbook_path, market_ticker, "orderbook")?;
    let temp_trades = create_filtered_csv(trades_path, market_ticker, "trades")?;

    // Create visualization tracker
    let viz_tracker = VisualizationTracker::new(time_window)?;

    // Create strategy
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

    // Run the backtest with visualization tracking and event logging
    let mut tracking_strategy = TrackingStrategy::new(strategy, viz_tracker);
    
    // Create the event stream ourselves to log all market events as they're processed
    let report = run_backtest_with_logging(&temp_orderbook, &temp_trades, &mut tracking_strategy, &config)?;

    // Clean up temporary files
    let _ = std::fs::remove_file(&temp_orderbook);
    let _ = std::fs::remove_file(&temp_trades);

    // Extract visualization data from the tracking strategy
    let viz_tracker = tracking_strategy.into_tracker();
    let viz_data = viz_tracker.finalize(report)?;

    Ok(viz_data)
}

fn create_filtered_csv(source_path: &PathBuf, market_ticker: &str, file_type: &str) -> Result<String> {
    let temp_filename = format!("temp_{}_{}.csv", file_type, market_ticker.replace("-", "_"));
    let mut reader = csv::ReaderBuilder::new().has_headers(true).from_path(source_path)?;
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

fn format_timestamp(ts: u64) -> String {
    // Convert timestamp to readable format
    // Assuming ts is Unix timestamp in seconds or milliseconds
    if ts > 1_000_000_000_000 {
        // Milliseconds
        format!("{}ms", ts % 1000)
    } else {
        // Seconds  
        format!("{}s", ts)
    }
}

/// Wrapper strategy that tracks all actions for visualization
struct TrackingStrategy {
    inner: Box<dyn KalshiStrategy>,
    tracker: VisualizationTracker,
}

impl TrackingStrategy {
    fn new(strategy: Box<dyn KalshiStrategy>, tracker: VisualizationTracker) -> Self {
        Self {
            inner: strategy,
            tracker,
        }
    }
    
    fn into_tracker(self) -> VisualizationTracker {
        self.tracker
    }
    
    fn set_shared_log(&mut self, log_writer: std::rc::Rc<std::cell::RefCell<std::fs::File>>) {
        self.tracker.set_shared_log(log_writer);
    }
}

impl KalshiStrategy for TrackingStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        // Record orderbook snapshot
        self.tracker.record_orderbook_snapshot(md);
        
        // Get strategy instructions
        let instructions = self.inner.on_book(md);
        
        // Record strategy actions
        for instr in &instructions {
            self.tracker.record_strategy_action(md.ts, instr.clone());
        }
        
        instructions
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Record fill
        self.tracker.record_fill(fill.clone());
        
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

/// Tracks data for visualization
struct VisualizationTracker {
    orderbook_snapshots: Vec<OrderbookSnapshot>,
    strategy_actions: Vec<StrategyAction>,
    fills: Vec<Fill>,
    time_window: u64,
    debug_log: Option<std::rc::Rc<std::cell::RefCell<std::fs::File>>>,
}

impl VisualizationTracker {
    fn new(time_window: u64) -> Result<Self> {
        Ok(Self {
            orderbook_snapshots: Vec::new(),
            strategy_actions: Vec::new(),
            fills: Vec::new(),
            time_window,
            debug_log: None,
        })
    }
    
    fn set_shared_log(&mut self, log_writer: std::rc::Rc<std::cell::RefCell<std::fs::File>>) {
        self.debug_log = Some(log_writer);
    }

    fn record_orderbook_snapshot(&mut self, md: &MarketData) {
        let snapshot = OrderbookSnapshot {
            ts: md.ts,
            bid: md.bid,
            ask: md.ask,
            spread: md.ask.saturating_sub(md.bid),
        };
        
        // Write debug info to shared log file
        if let Some(ref log_writer) = self.debug_log {
            if let Ok(mut log) = log_writer.try_borrow_mut() {
                let _ = writeln!(
                    log,
                    "SNAPSHOT: ts={}, ticker={}, bid={}¢, ask={}¢, spread={}¢",
                    md.ts, md.ticker, md.bid, md.ask, snapshot.spread
                );
                let _ = log.flush();
            }
        }
        
        self.orderbook_snapshots.push(snapshot);
    }

    fn record_strategy_action(&mut self, ts: u64, instr: OrderInstr) {
        let action = match instr {
            OrderInstr::Limit { side, price, qty, .. } => {
                if let Some(ref log_writer) = self.debug_log {
                    if let Ok(mut log) = log_writer.try_borrow_mut() {
                        let _ = writeln!(
                            log,
                            "ORDER: ts={}, side={:?}, price={}¢, qty={}",
                            ts, side, price, qty
                        );
                        let _ = log.flush();
                    }
                }
                StrategyAction::PlaceOrder {
                    ts,
                    side,
                    price,
                    qty,
                }
            },
            OrderInstr::Cancel { id } => {
                if let Some(ref log_writer) = self.debug_log {
                    if let Ok(mut log) = log_writer.try_borrow_mut() {
                        let _ = writeln!(log, "CANCEL: ts={}, order_id={}", ts, id);
                        let _ = log.flush();
                    }
                }
                StrategyAction::CancelOrder { ts, id }
            },
        };
        self.strategy_actions.push(action);
    }

    fn record_fill(&mut self, fill: Fill) {
        if let Some(ref log_writer) = self.debug_log {
            if let Ok(mut log) = log_writer.try_borrow_mut() {
                let _ = writeln!(
                    log,
                    "FILL: ts={}, order_id={}, price={}¢, qty={}",
                    fill.ts, fill.id, fill.price, fill.qty
                );
                let _ = log.flush();
            }
        }
        self.fills.push(fill);
    }



    fn finalize(mut self, _report: Report) -> Result<VisualizationData> {
        // Write summary to shared debug log
        if let Some(ref log_writer) = self.debug_log {
            if let Ok(mut log) = log_writer.try_borrow_mut() {
                let _ = writeln!(log, "\n=== SUMMARY ===");
                let _ = writeln!(log, "Total snapshots collected: {}", self.orderbook_snapshots.len());
                let _ = writeln!(log, "Total strategy actions: {}", self.strategy_actions.len());
                let _ = writeln!(log, "Total fills: {}", self.fills.len());
                
                if !self.orderbook_snapshots.is_empty() {
                    let first_ts = self.orderbook_snapshots.first().unwrap().ts;
                    let last_ts = self.orderbook_snapshots.last().unwrap().ts;
                    let _ = writeln!(log, "Time range: {} to {} (duration: {})", 
                                  first_ts, last_ts, last_ts - first_ts);
                }
            }
        }
        
        // If time window is specified and we have fills, filter data to that window
        if self.time_window > 0 && !self.fills.is_empty() {
            let first_fill_time = self.fills.first().unwrap().ts;
            let last_fill_time = self.fills.last().unwrap().ts;
            let start_time = first_fill_time.saturating_sub(self.time_window);
            let end_time = last_fill_time + self.time_window;

            if let Some(ref log_writer) = self.debug_log {
                if let Ok(mut log) = log_writer.try_borrow_mut() {
                    let _ = writeln!(log, "\nApplying time window filter:");
                    let _ = writeln!(log, "First fill: {}, Last fill: {}", first_fill_time, last_fill_time);
                    let _ = writeln!(log, "Filter range: {} to {}", start_time, end_time);
                }
            }

            // Filter orderbook snapshots
            let orig_snapshots = self.orderbook_snapshots.len();
            self.orderbook_snapshots.retain(|snap| snap.ts >= start_time && snap.ts <= end_time);
            
            // Filter strategy actions  
            let orig_actions = self.strategy_actions.len();
            self.strategy_actions.retain(|action| {
                let action_ts = match action {
                    StrategyAction::PlaceOrder { ts, .. } => *ts,
                    StrategyAction::CancelOrder { ts, .. } => *ts,
                };
                action_ts >= start_time && action_ts <= end_time
            });
            
            if let Some(ref log_writer) = self.debug_log {
                if let Ok(mut log) = log_writer.try_borrow_mut() {
                    let _ = writeln!(log, "Filtered snapshots: {} -> {}", orig_snapshots, self.orderbook_snapshots.len());
                    let _ = writeln!(log, "Filtered actions: {} -> {}", orig_actions, self.strategy_actions.len());
                }
            }
        }

        if let Some(ref log_writer) = self.debug_log {
            if let Ok(mut log) = log_writer.try_borrow_mut() {
                let _ = writeln!(log, "\nDebug log written to kalshi_debug.log");
                let _ = log.flush();
            }
        }

        Ok(VisualizationData {
            orderbook_snapshots: self.orderbook_snapshots,
            strategy_actions: self.strategy_actions,
            fills: self.fills,
        })
    }
}

/// Run backtest with event logging for debugging  
fn run_backtest_with_logging(
    orderbook_path: &str,
    trades_path: &str,
    strategy: &mut TrackingStrategy,
    config: &SimConfig,
) -> Result<Report> {
    // Create the event stream using the existing Kalshi modules
    let orderbook_iter = parse_orderbook(orderbook_path)?;
    let trades_iter = parse_trades(trades_path)?;
    let event_stream = merge(orderbook_iter, trades_iter);
    
    // Create a shared log file that we'll pass to the strategy
    let mut shared_log = std::fs::File::create("kalshi_debug.log")?;
    writeln!(shared_log, "=== KALSHI ORDERBOOK DEBUG LOG ===")?;
    writeln!(shared_log, "Events will be logged in chronological order as processed...\n")?;
    
    // Create a logging wrapper that writes trade events as they're processed
    use std::rc::Rc;
    use std::cell::RefCell;
    let log_writer = Rc::new(RefCell::new(shared_log));
    let log_writer_clone = log_writer.clone();
    
    let logged_events = event_stream.map(move |event_result| {
        event_result.map(|event| {
            // Log trade events as they're about to be processed by the engine
            if let Event::Trade { seq, ts, ticker, yes_price, no_price, qty, taker } = &event {
                if let Ok(mut log) = log_writer_clone.try_borrow_mut() {
                    let _ = writeln!(log, "MARKET_TRADE: ts={}, seq={}, ticker={}, yes_price={}¢, no_price={}¢, qty={}, taker={:?}",
                        ts, seq, ticker, yes_price, no_price, qty, taker);
                    let _ = log.flush();
                }
            }
            event
        })
    });
    
    // Give the strategy access to the same log writer
    strategy.set_shared_log(log_writer);
    
    // Run the backtest with the logged event stream
    run_backtest(config.clone(), strategy, logged_events)
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