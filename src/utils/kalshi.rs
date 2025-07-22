use anyhow::{Result, anyhow};
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::connect_async;
use reqwest::Client;
use serde_json::Value;
use csv::Writer;
use std::fs::File;
use std::time::{SystemTime, UNIX_EPOCH};
use base64::{engine::general_purpose, Engine as _};
use openssl::rsa::Rsa;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;

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

/// Fetch list of market tickers matching prefix
pub async fn fetch_market_tickers(api_key: &str, private_key_path: &str, prefix: &str) -> Result<Vec<String>> {
    let client = Client::new();
    let path = "/trade-api/v2/markets";
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let message = format!("{}GET{}", timestamp, path);
    let signature = sign_request(private_key_path, &message)?;

    let url = format!("https://api.elections.kalshi.com{}?limit=1000", path);
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

    let tickers = markets
        .iter()
        .filter_map(|m| m.get("ticker").and_then(|t| t.as_str()))
        .filter(|t| t.starts_with(prefix))
        .map(|s| s.to_string())
        .collect();

    Ok(tickers)
}

/// Collect orderbook deltas and trades for tickers prefix and write to CSV files
pub async fn collect_ws_data(api_key: &str, private_key_path: &str, prefix: &str, output_dir: &str) -> Result<()> {
    let tickers = fetch_market_tickers(api_key, private_key_path, prefix).await?;
    if tickers.is_empty() {
        return Err(anyhow!("no markets found with prefix {}", prefix));
    }

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

    // open csv writers
    let mut orderbook_writer = Writer::from_writer(File::create(format!("{}/orderbook.csv", output_dir))?);
    orderbook_writer.write_record(["ts","market_ticker","seq","price","delta","side","msg_type"])?;
    let mut trade_writer = Writer::from_writer(File::create(format!("{}/trades.csv", output_dir))?);
    trade_writer.write_record(["ts","market_ticker","yes_price","no_price","count","taker_side"])?;

    let subscribe = serde_json::json!({
        "id":1,
        "cmd":"subscribe",
        "params":{
            "channels":["orderbook_delta","trade"],
            "market_tickers": tickers
        }
    });
    write.send(Message::Text(subscribe.to_string())).await?;

    while let Some(msg) = read.next().await {
        let msg = msg?;
        if let Message::Text(text) = msg {
            let v: Value = serde_json::from_str(&text)?;
            if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
                match t {
                    "orderbook_snapshot" => {
                        if let Some(seq) = v.get("seq").and_then(|x| x.as_i64()) {
                            if let Some(msg) = v.get("msg") {
                                let ticker = msg.get("market_ticker").and_then(|x| x.as_str()).unwrap_or("");
                                if let Some(side_yes) = msg.get("yes").and_then(|x| x.as_array()) {
                                    for level in side_yes {
                                        if let Some(arr) = level.as_array() {
                                            if let (Some(price), Some(size)) = (arr.get(0).and_then(|x| x.as_i64()), arr.get(1).and_then(|x| x.as_i64())) {
                                                orderbook_writer.write_record(&[
                                                    timestamp.to_string(),
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
                                                orderbook_writer.write_record(&[
                                                    timestamp.to_string(),
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
                                orderbook_writer.flush()?;
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
                                orderbook_writer.write_record(&[
                                    timestamp.to_string(),
                                    ticker.to_string(),
                                    seq.to_string(),
                                    price.to_string(),
                                    delta.to_string(),
                                    side.to_string(),
                                    "delta".into()])?;
                                orderbook_writer.flush()?;
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
                            let ts = msg.get("ts").and_then(|x| x.as_i64()).unwrap_or(0);
                            trade_writer.write_record(&[
                                ts.to_string(),
                                ticker.to_string(),
                                yes_price.to_string(),
                                no_price.to_string(),
                                count.to_string(),
                                taker_side.to_string()])?;
                            trade_writer.flush()?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
