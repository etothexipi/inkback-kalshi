use plotly::{Scatter, Layout, Plot};
use crate::kalshi_types::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct VisualizationData {
    pub orderbook_snapshots: Vec<OrderbookSnapshot>,
    pub strategy_actions: Vec<StrategyAction>,
    pub fills: Vec<Fill>,
}

#[derive(Debug, Clone)]
pub struct OrderbookSnapshot {
    pub ts: u64,
    pub bid: u8,
    pub ask: u8,
    pub spread: u8,
}

#[derive(Debug, Clone)]
pub enum StrategyAction {
    PlaceOrder { ts: u64, side: Side, price: u8, qty: i64 },
    CancelOrder { ts: u64, id: OrderId },
}

pub struct OrderbookVisualizer {
    data: VisualizationData,
}

impl OrderbookVisualizer {
    pub fn new(data: VisualizationData) -> Self {
        Self { data }
    }

    pub fn create_orderbook_chart(&self) -> Plot {
        let mut traces = Vec::new();

        if self.data.orderbook_snapshots.is_empty() {
            // Create empty chart with message
            let layout = Layout::new()
                .title("No orderbook data available".into())
                .x_axis(plotly::layout::Axis::new().title("Time".into()))
                .y_axis(plotly::layout::Axis::new().title("Price (cents)".into()));
            
            let mut plot = Plot::new();
            plot.add_trace(Scatter::new(vec![] as Vec<f64>, vec![] as Vec<f64>));
            plot.set_layout(layout);
            return plot;
        }

        // Extract time and price data
        let times: Vec<u64> = self.data.orderbook_snapshots.iter().map(|s| s.ts).collect();
        let bids: Vec<u8> = self.data.orderbook_snapshots.iter().map(|s| s.bid).collect();
        let asks: Vec<u8> = self.data.orderbook_snapshots.iter().map(|s| s.ask).collect();
        let spreads: Vec<u8> = self.data.orderbook_snapshots.iter().map(|s| s.spread).collect();

        // Bid line (green)
        let bid_trace = Scatter::new(times.clone(), bids)
            .name("Bid")
            .line(plotly::common::Line::new().color("green").width(2.0))
            .mode(plotly::common::Mode::Lines);

        // Ask line (red)
        let ask_trace = Scatter::new(times.clone(), asks)
            .name("Ask")
            .line(plotly::common::Line::new().color("red").width(2.0))
            .mode(plotly::common::Mode::Lines);

        traces.push(bid_trace);
        traces.push(ask_trace);

        // Add spread area (light blue)
        let spread_trace = Scatter::new(times, spreads)
            .name("Spread")
            .line(plotly::common::Line::new().color("lightblue").width(1.0))
            .fill(plotly::common::Fill::ToZeroY)
            .opacity(0.3)
            .mode(plotly::common::Mode::Lines);

        traces.push(spread_trace);

        // Add fills if any
        if !self.data.fills.is_empty() {
            let fill_times: Vec<u64> = self.data.fills.iter().map(|f| f.ts).collect();
            let fill_prices: Vec<u8> = self.data.fills.iter().map(|f| f.price).collect();
            let fill_quantities: Vec<i64> = self.data.fills.iter().map(|f| f.qty).collect();

            let fill_trace = Scatter::new(fill_times, fill_prices)
                .name("Fills")
                .mode(plotly::common::Mode::Markers)
                .marker(plotly::common::Marker::new()
                    .color("yellow")
                    .size_array(fill_quantities.iter().map(|&q| (q as f64 / 10.0).max(5.0) as usize).collect())
                    .line(plotly::common::Line::new().color("orange").width(1.0)));

            traces.push(fill_trace);
        }

        // Add strategy orders
        let mut buy_orders = Vec::new();
        let mut sell_orders = Vec::new();
        let mut cancel_times = Vec::new();

        for action in &self.data.strategy_actions {
            match action {
                StrategyAction::PlaceOrder { ts, side, price, .. } => {
                    match side {
                        Side::Yes => buy_orders.push((*ts, *price)),
                        Side::No => sell_orders.push((*ts, *price)),
                    }
                }
                StrategyAction::CancelOrder { ts, .. } => {
                    cancel_times.push(*ts);
                }
            }
        }

        // Buy orders (green triangles)
        if !buy_orders.is_empty() {
            let (buy_times, buy_prices): (Vec<u64>, Vec<u8>) = buy_orders.into_iter().unzip();
            let buy_trace = Scatter::new(buy_times, buy_prices)
                .name("Buy Orders")
                .mode(plotly::common::Mode::Markers)
                .marker(plotly::common::Marker::new()
                    .color("lightgreen")
                    .symbol(plotly::common::MarkerSymbol::TriangleUp)
                    .size(10));

            traces.push(buy_trace);
        }

        // Sell orders (orange triangles)
        if !sell_orders.is_empty() {
            let (sell_times, sell_prices): (Vec<u64>, Vec<u8>) = sell_orders.into_iter().unzip();
            let sell_trace = Scatter::new(sell_times, sell_prices)
                .name("Sell Orders")
                .mode(plotly::common::Mode::Markers)
                .marker(plotly::common::Marker::new()
                    .color("orange")
                    .symbol(plotly::common::MarkerSymbol::TriangleDown)
                    .size(10));

            traces.push(sell_trace);
        }

        // Cancel orders (magenta vertical lines)
        if !cancel_times.is_empty() {
            // Create vertical lines for cancels
            let cancel_trace = Scatter::new(cancel_times.clone(), vec![0; cancel_times.len()])
                .name("Cancels")
                .mode(plotly::common::Mode::Markers)
                .marker(plotly::common::Marker::new()
                    .color("magenta")
                    .symbol(plotly::common::MarkerSymbol::X)
                    .size(8));

            traces.push(cancel_trace);
        }

        // Create layout
        let layout = Layout::new()
            .title("Kalshi Orderbook Visualization".into())
            .x_axis(plotly::layout::Axis::new()
                .title("Time".into())
                .show_grid(true))
            .y_axis(plotly::layout::Axis::new()
                .title("Price (cents)".into())
                .show_grid(true))
            .show_legend(true)
            .hover_mode(plotly::layout::HoverMode::Closest);

        let mut plot = Plot::new();
        for trace in traces {
            plot.add_trace(trace);
        }
        plot.set_layout(layout);
        plot
    }

    pub fn create_statistics_chart(&self) -> Plot {
        let mut traces = Vec::new();

        if self.data.orderbook_snapshots.is_empty() {
            let layout = Layout::new()
                .title("No data available for statistics".into());
            let mut plot = Plot::new();
            plot.add_trace(Scatter::new(vec![] as Vec<f64>, vec![] as Vec<f64>));
            plot.set_layout(layout);
            return plot;
        }

        // Calculate statistics over time
        let mut spread_stats = Vec::new();
        let mut time_windows = Vec::new();

        // Group data into time windows (e.g., every 100 snapshots)
        let window_size = (self.data.orderbook_snapshots.len() / 10).max(1);
        
        for (i, window) in self.data.orderbook_snapshots.chunks(window_size).enumerate() {
            let avg_spread: f64 = window.iter().map(|s| s.spread as f64).sum::<f64>() / window.len() as f64;
            let min_spread = window.iter().map(|s| s.spread).min().unwrap_or(0) as f64;
            let max_spread = window.iter().map(|s| s.spread).max().unwrap_or(0) as f64;
            
            spread_stats.push((avg_spread, min_spread, max_spread));
            time_windows.push(i as f64);
        }

        // Average spread over time
        let avg_spreads: Vec<f64> = spread_stats.iter().map(|(avg, _, _)| *avg).collect();
        let min_spreads: Vec<f64> = spread_stats.iter().map(|(_, min, _)| *min).collect();
        let max_spreads: Vec<f64> = spread_stats.iter().map(|(_, _, max)| *max).collect();

        let avg_spread_trace = Scatter::new(time_windows.clone(), avg_spreads)
            .name("Avg Spread")
            .line(plotly::common::Line::new().color("blue").width(2.0))
            .mode(plotly::common::Mode::Lines);

        let min_spread_trace = Scatter::new(time_windows.clone(), min_spreads)
            .name("Min Spread")
            .line(plotly::common::Line::new().color("green").width(1.0))
            .mode(plotly::common::Mode::Lines);

        let max_spread_trace = Scatter::new(time_windows, max_spreads)
            .name("Max Spread")
            .line(plotly::common::Line::new().color("red").width(1.0))
            .mode(plotly::common::Mode::Lines);

        traces.push(avg_spread_trace);
        traces.push(min_spread_trace);
        traces.push(max_spread_trace);

        // Add volume statistics if we have fills
        if !self.data.fills.is_empty() {
            let mut volume_by_window = HashMap::new();
            
            for fill in &self.data.fills {
                let window_idx = (fill.ts - self.data.orderbook_snapshots.first().unwrap().ts) / 1000; // 1 second windows
                *volume_by_window.entry(window_idx).or_insert(0) += fill.qty;
            }

            let mut volume_times = Vec::new();
            let mut volumes = Vec::new();
            
            for (time, volume) in volume_by_window {
                volume_times.push(time as f64);
                volumes.push(volume as f64);
            }

            let volume_trace = Scatter::new(volume_times, volumes)
                .name("Volume")
                .line(plotly::common::Line::new().color("purple").width(2.0))
                .mode(plotly::common::Mode::Lines)
                .y_axis("y2");

            traces.push(volume_trace);
        }

        let layout = Layout::new()
            .title("Orderbook Statistics Over Time".into())
            .x_axis(plotly::layout::Axis::new()
                .title("Time Window".into())
                .show_grid(true))
            .y_axis(plotly::layout::Axis::new()
                .title("Spread (cents)".into())
                .show_grid(true))
            .y_axis2(plotly::layout::Axis::new()
                .title("Volume".into())
                .overlaying("y")
                .side(plotly::common::AxisSide::Right))
            .show_legend(true);

        let mut plot = Plot::new();
        for trace in traces {
            plot.add_trace(trace);
        }
        plot.set_layout(layout);
        plot
    }

    pub fn generate_html_report(&self, output_path: &str) -> anyhow::Result<()> {
        let orderbook_chart = self.create_orderbook_chart();
        let stats_chart = self.create_statistics_chart();

        // Generate HTML content with embedded plotly data
        let orderbook_json = serde_json::to_string(&orderbook_chart)?;
        let stats_json = serde_json::to_string(&stats_chart)?;

        let avg_spread = if !self.data.orderbook_snapshots.is_empty() {
            let avg_spread: f64 = self.data.orderbook_snapshots.iter().map(|s| s.spread as f64).sum::<f64>() / self.data.orderbook_snapshots.len() as f64;
            format!("{:.1}", avg_spread)
        } else {
            "0.0".to_string()
        };

        let additional_metrics = if !self.data.fills.is_empty() {
            let total_volume: i64 = self.data.fills.iter().map(|f| f.qty).sum();
            let avg_fill_price: f64 = self.data.fills.iter().map(|f| f.price as f64).sum::<f64>() / self.data.fills.len() as f64;
            format!(
                r#"<div class="metric">
                    <strong>{}</strong>
                    Total Volume
                </div>
                <div class="metric">
                    <strong>{:.1}¢</strong>
                    Average Fill Price
                </div>"#,
                total_volume, avg_fill_price
            )
        } else {
            "".to_string()
        };

        let html_content = format!(
            r#"
<!DOCTYPE html>
<html>
<head>
    <title>Kalshi Orderbook Visualization</title>
    <script src="https://cdn.plot.ly/plotly-latest.min.js"></script>
    <style>
        body {{
            font-family: Arial, sans-serif;
            margin: 20px;
            background-color: #f5f5f5;
        }}
        .container {{
            max-width: 1200px;
            margin: 0 auto;
            background-color: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
        .chart {{
            margin: 20px 0;
            border: 1px solid #ddd;
            border-radius: 4px;
            height: 500px;
        }}
        .stats {{
            background-color: #f9f9f9;
            padding: 15px;
            border-radius: 4px;
            margin: 20px 0;
        }}
        h1, h2 {{
            color: #333;
        }}
        .metric {{
            display: inline-block;
            margin: 10px 20px 10px 0;
            padding: 10px;
            background-color: #e8f4fd;
            border-radius: 4px;
            min-width: 120px;
        }}
        .metric strong {{
            display: block;
            font-size: 18px;
            color: #0066cc;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🎯 Kalshi Orderbook Visualization</h1>
        
        <div class="stats">
            <h2>📊 Summary Statistics</h2>
            <div class="metric">
                <strong>{}</strong>
                Orderbook Snapshots
            </div>
            <div class="metric">
                <strong>{}</strong>
                Strategy Actions
            </div>
            <div class="metric">
                <strong>{}</strong>
                Fills
            </div>
            <div class="metric">
                <strong>{}¢</strong>
                Average Spread
            </div>
            {}
        </div>

        <h2>📈 Orderbook Dynamics</h2>
        <div class="chart" id="orderbook-chart"></div>
        
        <h2>📊 Statistics Over Time</h2>
        <div class="chart" id="stats-chart"></div>
    </div>

    <script>
        const orderbookChart = {orderbook_json};
        const statsChart = {stats_json};
        
        Plotly.newPlot('orderbook-chart', orderbookChart.data, orderbookChart.layout);
        Plotly.newPlot('stats-chart', statsChart.data, statsChart.layout);
    </script>
</body>
</html>
"#,
            self.data.orderbook_snapshots.len(),
            self.data.strategy_actions.len(),
            self.data.fills.len(),
            avg_spread,
            additional_metrics
        );

        // Replace placeholders with actual JSON data
        let html_content = html_content
            .replace("const orderbookChart = {};", &format!("const orderbookChart = {};", orderbook_json))
            .replace("const statsChart = {};", &format!("const statsChart = {};", stats_json));

        // Write HTML to file
        std::fs::write(output_path, html_content)?;
        println!("📊 HTML report generated: {}", output_path);
        
        Ok(())
    }
}

pub fn run_visualizer(data: VisualizationData) -> anyhow::Result<()> {
    let visualizer = OrderbookVisualizer::new(data);
    
    // Generate HTML report
    let output_path = "kalshi_visualization.html";
    visualizer.generate_html_report(output_path)?;
    
    println!("🚀 Visualization complete! Open '{}' in your web browser to view the interactive charts.", output_path);
    
    Ok(())
} 