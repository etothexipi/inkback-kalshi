use anyhow::Result;
use clap::Parser;
use dotenvy::dotenv;
use std::env;
use std::fs;

#[path = "../utils/kalshi.rs"]
mod kalshi;

#[derive(Parser)]
#[command(name = "kalshi_collector")]
#[command(about = "Collect market data from Kalshi")]
struct Args {
    /// Market ticker or prefix to collect data for
    market_input: Option<String>,
    
    /// Path to a text file containing a list of market tickers (one per line)
    #[arg(long)]
    markets_file: Option<String>,
}

/// Read market tickers from a text file
fn read_markets_from_file(file_path: &str) -> Result<Vec<String>> {
    let content = fs::read_to_string(file_path)?;
    let tickers: Vec<String> = content
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#')) // Skip empty lines and comments
        .collect();
    
    if tickers.is_empty() {
        return Err(anyhow::anyhow!("No valid market tickers found in file: {}", file_path));
    }
    
    println!("Read {} market tickers from file: {}", tickers.len(), file_path);
    for ticker in &tickers {
        println!("  {}", ticker);
    }
    
    Ok(tickers)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let api_key = env::var("KALSHI_API_KEY")?;
    let private_key_file = env::var("KALSHI_PRIVATE_KEY_FILE")?;
    
    let args = Args::parse();
    
    // Determine which input method to use
    let tickers = match (args.market_input, args.markets_file) {
        (Some(market_input), None) => {
            // Use the original functionality - fetch tickers based on input
            kalshi::fetch_market_tickers(&api_key, &private_key_file, &market_input).await?
        },
        (None, Some(markets_file)) => {
            // Read tickers from file
            read_markets_from_file(&markets_file)?
        },
        (Some(_), Some(_)) => {
            return Err(anyhow::anyhow!("Cannot specify both market_input and --markets-file. Use only one."));
        },
        (None, None) => {
            return Err(anyhow::anyhow!("Either market_input or --markets-file is required. Usage: kalshi_collector <MARKET_TICKER_OR_PREFIX> or kalshi_collector --markets-file <FILE_PATH>"));
        }
    };
    
    if tickers.is_empty() {
        return Err(anyhow::anyhow!("No markets found to collect data for"));
    }
    
    // Use the existing collect_ws_data function but pass the tickers directly
    kalshi::collect_ws_data_with_tickers(&api_key, &private_key_file, &tickers, "src/data").await?;
    Ok(())
}
