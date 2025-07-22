use anyhow::Result;
use dotenvy::dotenv;
use std::env;

#[path = "../utils/kalshi.rs"]
mod kalshi;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let api_key = env::var("KALSHI_API_KEY")?;
    let private_key_file = env::var("KALSHI_PRIVATE_KEY_FILE")?;
    
    let market_input = env::args().nth(1).ok_or_else(|| {
        anyhow::anyhow!("Market ticker or prefix is required. Usage: kalshi_collector <MARKET_TICKER_OR_PREFIX>")
    })?;
    
    kalshi::collect_ws_data(&api_key, &private_key_file, &market_input, "src/data").await?;
    Ok(())
}
