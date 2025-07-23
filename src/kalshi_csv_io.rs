use crate::kalshi_types::{RawObRow, RawTradeRow, Side};
use anyhow::Result;
use csv::ReaderBuilder;
use std::io::Read;
use std::fs::File;

/// Parse orderbook CSV file and return iterator over rows
pub fn parse_orderbook(path: &str) -> Result<impl Iterator<Item = Result<RawObRow>>> {
    let file = File::open(path)?;
    Ok(OrderbookIterator::new(file)?)
}

/// Parse trades CSV file and return iterator over rows  
pub fn parse_trades(path: &str) -> Result<impl Iterator<Item = Result<RawTradeRow>>> {
    let file = File::open(path)?;
    Ok(TradesIterator::new(file)?)
}

/// Iterator for orderbook CSV rows
pub struct OrderbookIterator<R: Read> {
    reader: csv::Reader<R>,
    headers: csv::StringRecord,
}

impl<R: Read> OrderbookIterator<R> {
    fn new(reader: R) -> Result<Self> {
        let mut csv_reader = ReaderBuilder::new()
            .has_headers(true)
            .from_reader(reader);
        
        let headers = csv_reader.headers()?.clone();
        
        Ok(Self {
            reader: csv_reader,
            headers,
        })
    }
}

impl<R: Read> Iterator for OrderbookIterator<R> {
    type Item = Result<RawObRow>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut record = csv::StringRecord::new();
        match self.reader.read_record(&mut record) {
            Ok(true) => Some(self.parse_orderbook_record(&record)),
            Ok(false) => None,
            Err(e) => Some(Err(e.into())),
        }
    }
}

impl<R: Read> OrderbookIterator<R> {
    fn parse_orderbook_record(&self, record: &csv::StringRecord) -> Result<RawObRow> {
        let seq = record.get(self.get_column_index("seq")?)
            .ok_or_else(|| anyhow::anyhow!("Missing seq column"))?
            .parse::<u64>()?;

        let ts = record.get(self.get_column_index("client_ts")?)
            .ok_or_else(|| anyhow::anyhow!("Missing client_ts column"))?
            .parse::<u64>()?;

        let ticker = record.get(self.get_column_index("ticker")?)
            .ok_or_else(|| anyhow::anyhow!("Missing ticker column"))?
            .to_string();

        let msg_type = record.get(self.get_column_index("msg_type")?)
            .ok_or_else(|| anyhow::anyhow!("Missing msg_type column"))?
            .to_string();

        let row = match msg_type.as_str() {
            "snapshot" => {
                // Parse snapshot data - expecting yes_levels and no_levels as JSON or structured format
                let yes_levels = self.parse_levels(record, "yes_levels")?;
                let no_levels = self.parse_levels(record, "no_levels")?;
                
                RawObRow {
                    seq,
                    ts,
                    ticker,
                    msg_type,
                    side: None,
                    price: None,
                    quantity: None,
                    yes_levels: Some(yes_levels),
                    no_levels: Some(no_levels),
                }
            }
            "delta" => {
                // Parse delta data
                let side_str = record.get(self.get_column_index("side")?)
                    .ok_or_else(|| anyhow::anyhow!("Missing side column for delta"))?;
                
                let side = match side_str.to_lowercase().as_str() {
                    "yes" => Side::Yes,
                    "no" => Side::No,
                    _ => return Err(anyhow::anyhow!("Invalid side: {}", side_str)),
                };

                let price = record.get(self.get_column_index("price")?)
                    .ok_or_else(|| anyhow::anyhow!("Missing price column for delta"))?
                    .parse::<u8>()?;

                let quantity = record.get(self.get_column_index("quantity")?)
                    .ok_or_else(|| anyhow::anyhow!("Missing quantity column for delta"))?
                    .parse::<i64>()?;

                RawObRow {
                    seq,
                    ts,
                    ticker,
                    msg_type,
                    side: Some(side),
                    price: Some(price),
                    quantity: Some(quantity),
                    yes_levels: None,
                    no_levels: None,
                }
            }
            _ => return Err(anyhow::anyhow!("Unknown msg_type: {}", msg_type)),
        };

        Ok(row)
    }

    fn get_column_index(&self, column_name: &str) -> Result<usize> {
        self.headers
            .iter()
            .position(|h| h == column_name)
            .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", column_name))
    }

    fn parse_levels(&self, record: &csv::StringRecord, column_name: &str) -> Result<Vec<(u8, i64)>> {
        let levels_str = record.get(self.get_column_index(column_name)?)
            .ok_or_else(|| anyhow::anyhow!("Missing {} column", column_name))?;

        if levels_str.is_empty() {
            return Ok(Vec::new());
        }

        // Try to parse as JSON first: [{"price": 50, "quantity": 100}, ...]
        if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(levels_str) {
            let mut levels = Vec::new();
            if let serde_json::Value::Array(arr) = json_value {
                for item in arr {
                    if let serde_json::Value::Object(obj) = item {
                        let price = obj.get("price")
                            .and_then(|v| v.as_u64())
                            .ok_or_else(|| anyhow::anyhow!("Invalid price in levels"))? as u8;
                        
                        let quantity = obj.get("quantity")
                            .and_then(|v| v.as_i64())
                            .ok_or_else(|| anyhow::anyhow!("Invalid quantity in levels"))?;
                        
                        levels.push((price, quantity));
                    }
                }
            }
            return Ok(levels);
        }

        // Try to parse as comma-separated pairs: "50:100,51:200"
        let mut levels = Vec::new();
        for pair in levels_str.split(',') {
            let parts: Vec<&str> = pair.split(':').collect();
            if parts.len() == 2 {
                let price = parts[0].trim().parse::<u8>()?;
                let quantity = parts[1].trim().parse::<i64>()?;
                levels.push((price, quantity));
            }
        }

        Ok(levels)
    }
}

/// Iterator for trades CSV rows
pub struct TradesIterator<R: Read> {
    reader: csv::Reader<R>,
    headers: csv::StringRecord,
}

impl<R: Read> TradesIterator<R> {
    fn new(reader: R) -> Result<Self> {
        let mut csv_reader = ReaderBuilder::new()
            .has_headers(true)
            .from_reader(reader);
        
        let headers = csv_reader.headers()?.clone();
        
        Ok(Self {
            reader: csv_reader,
            headers,
        })
    }
}

impl<R: Read> Iterator for TradesIterator<R> {
    type Item = Result<RawTradeRow>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut record = csv::StringRecord::new();
        match self.reader.read_record(&mut record) {
            Ok(true) => Some(self.parse_trade_record(&record)),
            Ok(false) => None,
            Err(e) => Some(Err(e.into())),
        }
    }
}

impl<R: Read> TradesIterator<R> {
    fn parse_trade_record(&self, record: &csv::StringRecord) -> Result<RawTradeRow> {
        let seq = record.get(self.get_column_index("seq")?)
            .ok_or_else(|| anyhow::anyhow!("Missing seq column"))?
            .parse::<u64>()?;

        let ts = record.get(self.get_column_index("client_ts")?)
            .ok_or_else(|| anyhow::anyhow!("Missing client_ts column"))?
            .parse::<u64>()?;

        let ticker = record.get(self.get_column_index("ticker")?)
            .ok_or_else(|| anyhow::anyhow!("Missing ticker column"))?
            .to_string();

        let yes_price = record.get(self.get_column_index("yes_price")?)
            .ok_or_else(|| anyhow::anyhow!("Missing yes_price column"))?
            .parse::<u8>()?;

        let no_price = record.get(self.get_column_index("no_price")?)
            .ok_or_else(|| anyhow::anyhow!("Missing no_price column"))?
            .parse::<u8>()?;

        let qty = record.get(self.get_column_index("quantity")?)
            .ok_or_else(|| anyhow::anyhow!("Missing quantity column"))?
            .parse::<i64>()?;

        let taker_str = record.get(self.get_column_index("taker")?)
            .ok_or_else(|| anyhow::anyhow!("Missing taker column"))?;

        let taker = match taker_str.to_lowercase().as_str() {
            "yes" => Side::Yes,
            "no" => Side::No,
            _ => return Err(anyhow::anyhow!("Invalid taker side: {}", taker_str)),
        };

        Ok(RawTradeRow {
            seq,
            ts,
            ticker,
            yes_price,
            no_price,
            qty,
            taker,
        })
    }

    fn get_column_index(&self, column_name: &str) -> Result<usize> {
        self.headers
            .iter()
            .position(|h| h == column_name)
            .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", column_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_orderbook_snapshot() {
        let csv_data = "seq,client_ts,ticker,msg_type,yes_levels,no_levels\n1,1640995200000,TEST-CONTRACT,snapshot,\"[{\"price\":50,\"quantity\":100}]\",\"[{\"price\":51,\"quantity\":200}]\"";
        let cursor = Cursor::new(csv_data);
        let mut iter = OrderbookIterator::new(cursor).unwrap();
        
        let row = iter.next().unwrap().unwrap();
        assert_eq!(row.seq, 1);
        assert_eq!(row.ticker, "TEST-CONTRACT");
        assert_eq!(row.msg_type, "snapshot");
        assert!(row.yes_levels.is_some());
        assert!(row.no_levels.is_some());
    }

    #[test]
    fn test_parse_orderbook_delta() {
        let csv_data = "seq,client_ts,ticker,msg_type,side,price,quantity\n2,1640995201000,TEST-CONTRACT,delta,yes,50,10";
        let cursor = Cursor::new(csv_data);
        let mut iter = OrderbookIterator::new(cursor).unwrap();
        
        let row = iter.next().unwrap().unwrap();
        assert_eq!(row.seq, 2);
        assert_eq!(row.ticker, "TEST-CONTRACT");
        assert_eq!(row.msg_type, "delta");
        assert_eq!(row.side, Some(Side::Yes));
        assert_eq!(row.price, Some(50));
        assert_eq!(row.quantity, Some(10));
    }

    #[test]
    fn test_parse_trades() {
        let csv_data = "seq,client_ts,ticker,yes_price,no_price,quantity,taker\n3,1640995202000,TEST-CONTRACT,50,50,25,yes";
        let cursor = Cursor::new(csv_data);
        let mut iter = TradesIterator::new(cursor).unwrap();
        
        let row = iter.next().unwrap().unwrap();
        assert_eq!(row.seq, 3);
        assert_eq!(row.ticker, "TEST-CONTRACT");
        assert_eq!(row.yes_price, 50);
        assert_eq!(row.no_price, 50);
        assert_eq!(row.qty, 25);
        assert_eq!(row.taker, Side::Yes);
    }
} 