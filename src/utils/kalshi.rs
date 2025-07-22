use anyhow::{Result, anyhow};
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::connect_async;
use reqwest::Client;
use serde_json::Value;
use csv::Writer;
use std::fs::{File, create_dir_all};
use std::time::{SystemTime, UNIX_EPOCH};
use base64::{engine::general_purpose, Engine as _};
use openssl::rsa::Rsa;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use chrono::{DateTime, Utc};

/// Sign Kalshi API request using RSA PSS SHA256
fn sign_request(private_key_pem: &str, message: &str) -> Result<String> {
    let key_pem = std::fs::read(private_key_pem)?;
    let rsa = Rsa::private_key_from_pem(&key_pem)?;
    let pkey = PKey::from_rsa(rsa)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey)?;
    signer.set_rsa_padding(openssl::rsa::Padding::PKCS1_PSS)?;
    signer.set_rsa_pss_saltlen(openssl::sign::RsaPssSaltlen::DIGEST_LENGTH)?;
    signer.update(message.as_bytes())?;
    let signature = signer.sign_to_vec()?;
    Ok(general_purpose::STANDARD.encode(signature))
}

/// Check if a single market ticker exists
async fn check_market_exists(api_key: &str, private_key_path: &str, ticker: &str) -> Result<bool> {
    let client = Client::new();
    let path = "/trade-api/v2/markets";
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let message = format!("{}GET{}", timestamp, path);
    let signature = sign_request(private_key_path, &message)?;

    let url = format!("https://api.elections.kalshi.com{}?tickers={}", path, ticker);
    
    let resp = client
        .get(&url)
        .header("KALSHI-ACCESS-KEY", api_key)
        .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
        .header("KALSHI-ACCESS-SIGNATURE", signature)
        .send()
        .await?;

    let body: Value = resp.json().await?;
    let markets = body
        .get("markets")
        .and_then(|v| v.as_array());
    
    Ok(markets.map_or(false, |m| !m.is_empty()))
}

/// Fetch list of market tickers - try exact ticker first, then prefix search
pub async fn fetch_market_tickers(api_key: &str, private_key_path: &str, input: &str) -> Result<Vec<String>> {
    // First, try treating the input as an exact ticker
    println!("Checking if '{}' is an exact market ticker...", input);
    
    match check_market_exists(api_key, private_key_path, input).await {
        Ok(true) => {
            println!("Found exact market: {}", input);
            return Ok(vec![input.to_string()]);
        },
        Ok(false) => {
            println!("Market '{}' not found, falling back to prefix search...", input);
        },
        Err(e) => {
            println!("Error checking exact market ({}), falling back to prefix search...", e);
        }
    }
    
    // Fallback to prefix search
    fetch_markets_by_prefix(api_key, private_key_path, input).await
}

/// Fetch list of market tickers matching prefix (original logic)
async fn fetch_markets_by_prefix(api_key: &str, private_key_path: &str, prefix: &str) -> Result<Vec<String>> {
    let client = Client::new();
    let mut all_tickers = Vec::new();
    let mut cursor: Option<String> = None;
    
    // Get current timestamp for filtering future markets
    let current_ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    
    loop {
        let path = "/trade-api/v2/markets";
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let message = format!("{}GET{}", timestamp, path);
        let signature = sign_request(private_key_path, &message)?;

        // Build URL with query parameters
        let mut url = format!("https://api.elections.kalshi.com{}?limit=1000&min_close_ts={}", path, current_ts);
        if let Some(ref cursor_val) = cursor {
            url.push_str(&format!("&cursor={}", cursor_val));
        }

        println!("Fetching markets from: {}", url);
        
        let resp = client
            .get(&url)
            .header("KALSHI-ACCESS-KEY", api_key)
            .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
            .header("KALSHI-ACCESS-SIGNATURE", signature)
            .send()
            .await?;

        let body: Value = resp.json().await?;
        let markets = body
            .get("markets")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("unexpected response"))?;

        println!("Fetched {} markets in this page", markets.len());
        
        // Extract tickers from this page
        let page_tickers: Vec<String> = markets
            .iter()
            .filter_map(|m| m.get("ticker").and_then(|t| t.as_str()))
            .map(|s| s.to_string())
            .collect();
        
        all_tickers.extend(page_tickers);
        
        // Check if there's a next page
        cursor = body.get("cursor").and_then(|c| c.as_str()).map(|s| s.to_string());
        if cursor.is_none() || cursor.as_ref().unwrap().is_empty() {
            break;
        }
        
        println!("Found cursor for next page, continuing...");
    }
    
    println!("Total markets fetched across all pages: {}", all_tickers.len());
    println!("Sample of all tickers (first 10):");
    for ticker in all_tickers.iter().take(10) {
        println!("  {}", ticker);
    }
    
    println!("Looking for markets with prefix: '{}'", prefix);
    
    let tickers: Vec<String> = all_tickers
        .iter()
        .filter(|t| t.starts_with(prefix))
        .map(|s| s.to_string())
        .collect();
    
    println!("Found {} markets matching prefix '{}'", tickers.len(), prefix);
    for ticker in &tickers {
        println!("  Matched: {}", ticker);
    }

    Ok(tickers)
}

/// Collect orderbook deltas and trades for tickers prefix and write to CSV files
pub async fn collect_ws_data(api_key: &str, private_key_path: &str, market_input: &str, output_dir: &str) -> Result<()> {
    let tickers = fetch_market_tickers(api_key, private_key_path, market_input).await?;
    if tickers.is_empty() {
        return Err(anyhow!("no markets found for input '{}'", market_input));
    }

    // Create output directory if it doesn't exist
    create_dir_all(output_dir)?;

    let path = "/trade-api/ws/v2";
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let message = format!("{}GET{}", timestamp, path);
    let signature = sign_request(private_key_path, &message)?;

    let req = tungstenite::client::IntoClientRequest::into_client_request(format!("wss://api.elections.kalshi.com{}", path))?;
    let mut req = req;
    req.headers_mut().append("KALSHI-ACCESS-KEY", api_key.parse()?);
    req.headers_mut().append("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string().parse()?);
    req.headers_mut().append("KALSHI-ACCESS-SIGNATURE", signature.parse()?);

    let (ws_stream, _) = connect_async(req).await?;
    let (mut write, mut read) = ws_stream.split();

    let subscribe = serde_json::json!({
        "id":1,
        "cmd":"subscribe",
        "params":{
            "channels":["orderbook_delta","trade"],
            "market_tickers": tickers
        }
    });
    write.send(Message::Text(subscribe.to_string())).await?;

    // Wait for first message to determine filename timestamp
    let mut orderbook_writer: Option<Writer<File>> = None;
    let mut trade_writer: Option<Writer<File>> = None;

    while let Some(msg) = read.next().await {
        let msg = msg?;
        if let Message::Text(text) = msg {
            // Record client reception timestamp in UTC for every message
            let client_timestamp_utc = Utc::now();
            let client_timestamp_millis = client_timestamp_utc.timestamp_millis() as u64;
            
            // Initialize CSV files on first message using the timestamp
            if orderbook_writer.is_none() {
                let date_suffix = client_timestamp_utc.format("%Y%m%d_%H%M%S").to_string();
                
                let orderbook_filename = format!("{}/orderbook_{}.csv", output_dir, date_suffix);
                let trade_filename = format!("{}/trades_{}.csv", output_dir, date_suffix);
                
                println!("Creating CSV files:");
                println!("  {}", orderbook_filename);
                println!("  {}", trade_filename);
                
                let mut ob_writer = Writer::from_writer(File::create(&orderbook_filename)?);
                ob_writer.write_record(["client_ts","market_ticker","seq","price","delta","side","msg_type"])?;
                orderbook_writer = Some(ob_writer);
                
                let mut tr_writer = Writer::from_writer(File::create(&trade_filename)?);
                tr_writer.write_record(["client_ts","server_ts","market_ticker","yes_price","no_price","count","taker_side"])?;
                trade_writer = Some(tr_writer);
            }
            
            let v: Value = serde_json::from_str(&text)?;
            if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
                match t {
                    "orderbook_snapshot" => {
                        if let Some(seq) = v.get("seq").and_then(|x| x.as_i64()) {
                            if let Some(msg) = v.get("msg") {
                                let ticker = msg.get("market_ticker").and_then(|x| x.as_str()).unwrap_or("");
                                let ob_writer = orderbook_writer.as_mut().unwrap();
                                
                                if let Some(side_yes) = msg.get("yes").and_then(|x| x.as_array()) {
                                    for level in side_yes {
                                        if let Some(arr) = level.as_array() {
                                            if let (Some(price), Some(size)) = (arr.get(0).and_then(|x| x.as_i64()), arr.get(1).and_then(|x| x.as_i64())) {
                                                ob_writer.write_record(&[
                                                    client_timestamp_millis.to_string(),
                                                    ticker.to_string(),
                                                    seq.to_string(),
                                                    price.to_string(),
                                                    size.to_string(),
                                                    "yes".into(),
                                                    "snapshot".into()])?;
                                            }
                                        }
                                    }
                                }
                                if let Some(side_no) = msg.get("no").and_then(|x| x.as_array()) {
                                    for level in side_no {
                                        if let Some(arr) = level.as_array() {
                                            if let (Some(price), Some(size)) = (arr.get(0).and_then(|x| x.as_i64()), arr.get(1).and_then(|x| x.as_i64())) {
                                                ob_writer.write_record(&[
                                                    client_timestamp_millis.to_string(),
                                                    ticker.to_string(),
                                                    seq.to_string(),
                                                    price.to_string(),
                                                    size.to_string(),
                                                    "no".into(),
                                                    "snapshot".into()])?;
                                            }
                                        }
                                    }
                                }
                                ob_writer.flush()?;
                            }
                        }
                    }
                    "orderbook_delta" => {
                        if let Some(seq) = v.get("seq").and_then(|x| x.as_i64()) {
                            if let Some(msg) = v.get("msg") {
                                let ticker = msg.get("market_ticker").and_then(|x| x.as_str()).unwrap_or("");
                                let price = msg.get("price").and_then(|x| x.as_i64()).unwrap_or(0);
                                let delta = msg.get("delta").and_then(|x| x.as_i64()).unwrap_or(0);
                                let side = msg.get("side").and_then(|x| x.as_str()).unwrap_or("");
                                let ob_writer = orderbook_writer.as_mut().unwrap();
                                
                                ob_writer.write_record(&[
                                    client_timestamp_millis.to_string(),
                                    ticker.to_string(),
                                    seq.to_string(),
                                    price.to_string(),
                                    delta.to_string(),
                                    side.to_string(),
                                    "delta".into()])?;
                                ob_writer.flush()?;
                            }
                        }
                    }
                    "trade" => {
                        if let Some(msg) = v.get("msg") {
                            let ticker = msg.get("market_ticker").and_then(|x| x.as_str()).unwrap_or("");
                            let yes_price = msg.get("yes_price").and_then(|x| x.as_i64()).unwrap_or(0);
                            let no_price = msg.get("no_price").and_then(|x| x.as_i64()).unwrap_or(0);
                            let count = msg.get("count").and_then(|x| x.as_i64()).unwrap_or(0);
                            let taker_side = msg.get("taker_side").and_then(|x| x.as_str()).unwrap_or("");
                            let server_ts = msg.get("ts").and_then(|x| x.as_i64()).unwrap_or(0);
                            let tr_writer = trade_writer.as_mut().unwrap();
                            
                            tr_writer.write_record(&[
                                client_timestamp_millis.to_string(),
                                server_ts.to_string(), // Server timestamp from trade message
                                ticker.to_string(),
                                yes_price.to_string(),
                                no_price.to_string(),
                                count.to_string(),
                                taker_side.to_string()])?;
                            tr_writer.flush()?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
