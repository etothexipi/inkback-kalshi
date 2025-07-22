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
    
    let prefix = env::args().nth(1).ok_or_else(|| {
        anyhow::anyhow!("Market prefix is required. Usage: kalshi_collector <MARKET_PREFIX>")
    })?;
    
    kalshi::collect_ws_data(&api_key, &private_key_file, &prefix, "src/data").await?;
    Ok(())
}
