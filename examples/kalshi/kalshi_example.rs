use inkback::kalshi_backtest::{KalshiBacktest, run_simple_backtest, calculate_performance_metrics};
use inkback::kalshi_backtest::kalshi_strategy::{KalshiStrategy, SpreadMmStrategy, SpreadMmParams, DirectionalStrategy};
use inkback::kalshi_backtest::kalshi_types::{SimConfig, Side, OrderInstr, Fill, MarketData};
use std::time::Duration;
use anyhow::Result;

/// Custom strategy that implements a momentum-based approach
struct MomentumStrategy {
    last_mid_price: Option<f64>,
    position_target: i64,
    current_position: i64,
    order_size: i64,
}

impl MomentumStrategy {
    fn new(order_size: i64) -> Self {
        Self {
            last_mid_price: None,
            position_target: 0,
            current_position: 0,
            order_size,
        }
    }
}

impl KalshiStrategy for MomentumStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        let current_mid = (md.bid as f64 + md.ask as f64) / 2.0;
        
        // Only trade if we have a previous price to compare
        if let Some(last_mid) = self.last_mid_price {
            let price_change = current_mid - last_mid;
            let change_threshold = 1.0; // 1 cent threshold
            
            // Update position target based on momentum
            if price_change > change_threshold {
                // Price going up, want to be long YES
                self.position_target = self.order_size;
            } else if price_change < -change_threshold {
                // Price going down, want to be long NO (short YES)
                self.position_target = -self.order_size;
            }
            
            // Calculate how much we need to trade
            let position_diff = self.position_target - self.current_position;
            
            if position_diff.abs() >= self.order_size / 2 {
                // Need to adjust position
                if position_diff > 0 {
                    // Need to buy YES
                    return vec![OrderInstr::Limit {
                        side: Side::Yes,
                        price: md.ask, // Market order (pay the spread)
                        qty: position_diff,
                        ttl: Duration::from_secs(10),
                    }];
                } else {
                    // Need to buy NO (sell YES)
                    return vec![OrderInstr::Limit {
                        side: Side::No,
                        price: md.bid, // Market order (pay the spread)
                        qty: -position_diff,
                        ttl: Duration::from_secs(10),
                    }];
                }
            }
        }
        
        self.last_mid_price = Some(current_mid);
        vec![]
    }

    fn on_fill(&mut self, fill: &Fill) {
        println!("Momentum strategy fill: {} contracts at {} cents", fill.qty, fill.price);
        
        // Update our position tracking (simplified)
        if fill.price < 50 {
            // Assume this was a YES buy
            self.current_position += fill.qty as i64;
        } else {
            // Assume this was a NO buy
            self.current_position -= fill.qty as i64;
        }
        
        println!("New position: {}", self.current_position);
    }
}

fn main() -> Result<()> {
    println!("🎯 Kalshi Backtesting Example\n");

    // Create sample data for demonstration
    create_sample_data()?;

    // Example 1: Spread Market Making Strategy
    println!("=== Example 1: Spread Market Making ===");
    
    let config = SimConfig {
        latency: Duration::from_millis(5), // 5ms latency
        start_time: 0,
        end_time: None,
    };

    let params = SpreadMmParams {
        min_spread: 3,      // Minimum 3 cent spread
        post_qty: 50,       // Post 50 contracts per side
        max_pos: 200,       // Maximum 200 contract position
        order_ttl: Duration::from_secs(10),
    };

    let mut spread_strategy = SpreadMmStrategy::new(params);
    let backtest = KalshiBacktest::new(config.clone());

    let result = backtest.run_from_files(
        "sample_orderbook.csv",
        "sample_trades.csv",
        &mut spread_strategy,
    )?;

    let perf = calculate_performance_metrics(&result);
    println!("{}\n", perf);

    // Example 2: Directional Strategy
    println!("=== Example 2: Directional Strategy (YES bias) ===");
    
    let mut directional_strategy = DirectionalStrategy::new(
        Side::Yes,  // Betting YES will happen
        100,        // Target 100 contracts
        60,         // Maximum 60 cents per contract
    );

    let result = backtest.run_from_files(
        "sample_orderbook.csv",
        "sample_trades.csv", 
        &mut directional_strategy,
    )?;

    let perf = calculate_performance_metrics(&result);
    println!("{}\n", perf);

    // Example 3: Custom Momentum Strategy
    println!("=== Example 3: Custom Momentum Strategy ===");
    
    let mut momentum_strategy = MomentumStrategy::new(75);

    let result = backtest.run_from_files(
        "sample_orderbook.csv",
        "sample_trades.csv",
        &mut momentum_strategy,
    )?;

    let perf = calculate_performance_metrics(&result);
    println!("{}\n", perf);

    // Example 4: Using the simple backtest function
    println!("=== Example 4: Simple Backtest Function ===");

    let mut simple_strategy = SpreadMmStrategy::new(SpreadMmParams::default());
    let result = run_simple_backtest(
        "sample_orderbook.csv",
        "sample_trades.csv",
        &mut simple_strategy,
        10, // 10ms latency
    )?;

    let perf = calculate_performance_metrics(&result);
    println!("{}\n", perf);

    // Example 5: Different latency scenarios
    println!("=== Example 5: Latency Impact Analysis ===");
    
    for latency_ms in [1, 5, 10, 25, 50] {
        let mut strategy = SpreadMmStrategy::new(SpreadMmParams::default());
        let result = run_simple_backtest(
            "sample_orderbook.csv",
            "sample_trades.csv",
            &mut strategy,
            latency_ms,
        )?;

        println!(
            "Latency {}ms: PnL ${:.2}, Volume {}, Orders {}",
            latency_ms,
            result.metrics.gross_pnl_cents as f64 / 100.0,
            result.metrics.volume_traded,
            result.metrics.total_orders
        );
    }

    println!("\n✅ Kalshi backtesting examples completed!");
    println!("📁 Check the generated sample_orderbook.csv and sample_trades.csv files");
    println!("💡 Modify the strategies above to test your own trading ideas");

    Ok(())
}

fn create_sample_data() -> Result<()> {
    use std::fs::File;
    use std::io::Write;

    // Create realistic orderbook data showing market dynamics
    let mut orderbook_file = File::create("sample_orderbook.csv")?;
    writeln!(orderbook_file, "seq,client_ts,ticker,msg_type,yes_levels,no_levels")?;
    
    // Initial snapshot
    writeln!(orderbook_file, "1,1640995200000,KXMLBGAME-25JUL22SDMIA-SD,snapshot,\"[{{\"price\":42,\"quantity\":150}},{{\"price\":41,\"quantity\":200}},{{\"price\":40,\"quantity\":300}}]\",\"[{{\"price\":58,\"quantity\":120}},{{\"price\":59,\"quantity\":180}},{{\"price\":60,\"quantity\":250}}]\"")?;
    
    // Market tightens
    writeln!(orderbook_file, "3,1640995202000,KXMLBGAME-25JUL22SDMIA-SD,snapshot,\"[{{\"price\":44,\"quantity\":100}},{{\"price\":43,\"quantity\":200}},{{\"price\":42,\"quantity\":250}}]\",\"[{{\"price\":56,\"quantity\":110}},{{\"price\":57,\"quantity\":190}},{{\"price\":58,\"quantity\":280}}]\"")?;
    
    // Even tighter spread 
    writeln!(orderbook_file, "5,1640995204000,KXMLBGAME-25JUL22SDMIA-SD,snapshot,\"[{{\"price\":46,\"quantity\":80}},{{\"price\":45,\"quantity\":150}},{{\"price\":44,\"quantity\":220}}]\",\"[{{\"price\":54,\"quantity\":90}},{{\"price\":55,\"quantity\":160}},{{\"price\":56,\"quantity\":240}}]\"")?;
    
    // Market moves up
    writeln!(orderbook_file, "7,1640995206000,KXMLBGAME-25JUL22SDMIA-SD,snapshot,\"[{{\"price\":48,\"quantity\":70}},{{\"price\":47,\"quantity\":140}},{{\"price\":46,\"quantity\":210}}]\",\"[{{\"price\":52,\"quantity\":80}},{{\"price\":53,\"quantity\":150}},{{\"price\":54,\"quantity\":220}}]\"")?;

    // Create corresponding trades data
    let mut trades_file = File::create("sample_trades.csv")?;
    writeln!(trades_file, "seq,client_ts,ticker,yes_price,no_price,quantity,taker")?;
    
    writeln!(trades_file, "2,1640995201000,KXMLBGAME-25JUL22SDMIA-SD,44,56,25,yes")?;
    writeln!(trades_file, "4,1640995203000,KXMLBGAME-25JUL22SDMIA-SD,46,54,40,yes")?;
    writeln!(trades_file, "6,1640995205000,KXMLBGAME-25JUL22SDMIA-SD,48,52,30,no")?;
    writeln!(trades_file, "8,1640995207000,KXMLBGAME-25JUL22SDMIA-SD,48,52,15,yes")?;

    println!("✅ Created sample data files with realistic market dynamics");
    Ok(())
} 