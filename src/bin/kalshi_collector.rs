use anyhow::Result;
use dotenvy::dotenv;
use std::env;

#[path = "../utils/kalshi.rs"]
mod kalshi;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let api_key = env::var("KALSHI_ACCESS_KEY")?;
    let private_key = env::var("KALSHI_PRIVATE_KEY")?;
    let prefix = env::args().nth(1).unwrap_or_else(|| "CPI".to_string());
    kalshi::collect_ws_data(&api_key, &private_key, &prefix, "src/data").await?;
    Ok(())
}
