use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Unique identifier for orders
pub type OrderId = u64;

/// Side of the market
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Yes,
    No,
}

/// Events in the unified stream
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Snapshot {
        seq: u64,
        ts: u64,
        ticker: String,
        yes_levels: Vec<(u8, i64)>, // (price_cents, quantity)
        no_levels: Vec<(u8, i64)>,
    },
    Delta {
        seq: u64,
        ts: u64,
        ticker: String,
        side: Side,
        price: u8, // cents
        delta: i64, // positive = add, negative = remove
    },
    Trade {
        seq: u64,
        ts: u64,
        ticker: String,
        yes_price: u8, // cents
        no_price: u8,  // cents
        qty: i64,
        taker: Side, // which side was the taker
    },
}

impl Event {
    pub fn seq(&self) -> u64 {
        match self {
            Event::Snapshot { seq, .. } => *seq,
            Event::Delta { seq, .. } => *seq,
            Event::Trade { seq, .. } => *seq,
        }
    }

    pub fn ts(&self) -> u64 {
        match self {
            Event::Snapshot { ts, .. } => *ts,
            Event::Delta { ts, .. } => *ts,
            Event::Trade { ts, .. } => *ts,
        }
    }

    pub fn ticker(&self) -> &str {
        match self {
            Event::Snapshot { ticker, .. } => ticker,
            Event::Delta { ticker, .. } => ticker,
            Event::Trade { ticker, .. } => ticker,
        }
    }
}

/// Market data provided to strategies
#[derive(Debug, Clone)]
pub struct MarketData {
    pub ts: u64,
    pub ticker: String,
    pub bid: u8,  // best bid price in cents
    pub ask: u8,  // best ask price in cents
}

/// Order instructions from strategy
#[derive(Debug, Clone)]
pub enum OrderInstr {
    Limit {
        side: Side,
        price: u8,
        qty: i64,
        ttl: Duration,
    },
    Cancel {
        id: OrderId,
    },
}

/// Active order in the simulator
#[derive(Debug, Clone)]
pub struct ActiveOrder {
    pub id: OrderId,
    pub side: Side,
    pub price: u8,
    pub remaining: i64,
    pub expiry_time: u64,
}

/// Pending order waiting for activation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOrder {
    pub id: OrderId,
    pub side: Side,
    pub price: u8,
    pub qty: i64,
    pub activation_time: u64,
    pub expiry_time: u64,
}

impl PendingOrder {
    pub fn into_active(self) -> ActiveOrder {
        ActiveOrder {
            id: self.id,
            side: self.side,
            price: self.price,
            remaining: self.qty,
            expiry_time: self.expiry_time,
        }
    }
}

/// Fill notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub id: OrderId,
    pub price: u8,
    pub qty: i64,
    pub ts: u64,
    pub side: Side,  // The side of the order that was filled
}

/// Order audit trail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAudit {
    pub id: OrderId,
    pub side: Side,
    pub price: u8,
    pub original_qty: i64,
    pub filled_qty: i64,
    pub create_time: u64,
    pub activation_time: u64,
    pub expiry_time: u64,
    pub status: OrderStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderStatus {
    Created,
    Active,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}

/// Metrics collected during backtest
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub gross_pnl_cents: i64,
    pub num_spreads: usize,
    pub volume_traded: i64,
    pub win_rate: f64,
    pub roi: f64,
    pub total_fills: usize,
    pub total_orders: usize,
    pub cancelled_orders: usize,
    pub expired_orders: usize,
}

impl Metrics {
    pub fn update_fill(&mut self, fill: &Fill, is_buy: bool, cost_basis: i64) {
        self.total_fills += 1;
        self.volume_traded += fill.qty;
        
        if !is_buy {
            // This is a sell, calculate PnL
            let pnl = (fill.price as i64 - cost_basis) * fill.qty;
            self.gross_pnl_cents += pnl;
            
            if pnl > 0 {
                self.num_spreads += 1;
            }
        }
    }

    pub fn calculate_win_rate(&mut self) {
        if self.num_spreads > 0 {
            // This is a simplified win rate calculation
            // In practice, you'd track individual round trips
            self.win_rate = 0.5; // Placeholder
        }
    }
}

/// Final backtest report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub metrics: Metrics,
    pub fills: Vec<Fill>,
    pub orders: Vec<OrderAudit>,
}

/// Configuration for the execution simulator
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub latency: Duration,
    pub start_time: u64,
    pub end_time: Option<u64>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(5),
            start_time: 0,
            end_time: None,
        }
    }
}

/// Raw orderbook row from CSV
#[derive(Debug, Clone)]
pub struct RawObRow {
    pub seq: u64,
    pub ts: u64,
    pub ticker: String,
    pub msg_type: String, // "snapshot" or "delta"
    pub side: Option<Side>,
    pub price: Option<u8>,
    pub quantity: Option<i64>,
    pub yes_levels: Option<Vec<(u8, i64)>>,
    pub no_levels: Option<Vec<(u8, i64)>>,
}

/// Raw trade row from CSV
#[derive(Debug, Clone)]
pub struct RawTradeRow {
    pub seq: u64,
    pub ts: u64,
    pub ticker: String,
    pub yes_price: u8,
    pub no_price: u8,
    pub qty: i64,
    pub taker: Side,
} 