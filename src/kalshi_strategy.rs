use crate::kalshi_types::{Fill, MarketData, OrderInstr, OrderId, Side};
use std::time::Duration;

/// Trait for Kalshi trading strategies
pub trait KalshiStrategy {
    /// Called when market data is updated
    /// Returns a vector of order instructions to execute
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr>;

    /// Called when an order is filled
    fn on_fill(&mut self, fill: &Fill);
}

/// Parameters for the spread market making strategy
#[derive(Debug, Clone)]
pub struct SpreadMmParams {
    pub min_spread: u8,       // Minimum spread in cents
    pub post_qty: i64,        // Quantity to post on each side
    pub max_pos: i64,         // Maximum position size
    pub order_ttl: Duration,  // Time to live for orders
}

impl Default for SpreadMmParams {
    fn default() -> Self {
        Self {
            min_spread: 4,
            post_qty: 50,
            max_pos: 100,
            order_ttl: Duration::from_secs(5),
        }
    }
}

/// Simple spread market making strategy
/// Posts quotes on both sides when spread is wide enough
#[derive(Debug)]
pub struct SpreadMmStrategy {
    params: SpreadMmParams,
    position: i64,           // Net position (positive = long YES, negative = long NO)
    active_orders: Vec<OrderId>, // Track active order IDs
    last_ticker: Option<String>,
    order_id_counter: u64,
}

impl SpreadMmStrategy {
    pub fn new(params: SpreadMmParams) -> Self {
        Self {
            params,
            position: 0,
            active_orders: Vec::new(),
            last_ticker: None,
            order_id_counter: 0,
        }
    }

    fn next_order_id(&mut self) -> OrderId {
        self.order_id_counter += 1;
        self.order_id_counter
    }

    /// Cancel all active orders
    fn cancel_all_orders(&mut self) -> Vec<OrderInstr> {
        let cancel_orders: Vec<OrderInstr> = self.active_orders
            .iter()
            .map(|&id| OrderInstr::Cancel { id })
            .collect();
        
        self.active_orders.clear();
        cancel_orders
    }

    /// Calculate optimal bid/ask prices based on current market and position
    fn calculate_quotes(&self, market_bid: u8, market_ask: u8) -> Option<(u8, u8)> {
        // Don't make markets in uninitialized/fake conditions
        if market_bid == 0 || market_ask == 100 {
            return None;
        }

        // Don't make markets when spread is unrealistically wide (>20 cents)
        let spread = market_ask.saturating_sub(market_bid);
        if spread > 20 {
            return None;
        }

        // Don't make markets at extreme prices - only allow exits
        if market_bid >= 99 || market_ask <= 1 {
            return None;
        }
        
        // Only quote if market spread is wide enough (min_spread must be >= 3)
        if spread < self.params.min_spread {
            return None;
        }

        // Calculate our quotes inside the current market
        let our_bid = market_bid + 1;
        let our_ask = market_ask - 1;

        // Don't place bids that could get filled at extreme prices
        // If our_bid >= 99, it would get filled when market crashes to 1¢
        if our_bid >= 99 {
            return None;
        }

        // Don't place asks that could get filled at extreme prices  
        // If our_ask <= 1, it would get filled when market spikes to 99¢
        if our_ask <= 1 {
            return None;
        }

        // Ensure our internal quotes maintain at least 1 cent spread
        if our_ask.saturating_sub(our_bid) < 1 {
            return None;
        }

        Some((our_bid, our_ask))
    }

    /// Determine whether to quote based on position limits
    fn should_quote_side(&self, side: Side) -> bool {
        match side {
            Side::Yes => {
                // Can buy YES (go long) if position < max_pos
                self.position < self.params.max_pos
            }
            Side::No => {
                // Can buy NO (go short YES) if position > -max_pos
                self.position > -self.params.max_pos
            }
        }
    }

    /// Adjust quantities based on current position
    fn calculate_quote_quantity(&self, side: Side) -> i64 {
        let base_qty = self.params.post_qty;
        
        match side {
            Side::Yes => {
                // Buying YES: reduce quantity as we get more long
                let remaining_capacity = self.params.max_pos - self.position;
                base_qty.min(remaining_capacity).max(0)
            }
            Side::No => {
                // Buying NO (shorting YES): reduce quantity as we get more short
                let remaining_capacity = self.position + self.params.max_pos;
                base_qty.min(remaining_capacity).max(0)
            }
        }
    }
}

impl KalshiStrategy for SpreadMmStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        let mut instructions = Vec::new();

        // Cancel existing orders if ticker changed
        if self.last_ticker.as_ref() != Some(&md.ticker) {
            instructions.extend(self.cancel_all_orders());
            self.last_ticker = Some(md.ticker.clone());
        }

        // Calculate optimal quotes
        let quotes = match self.calculate_quotes(md.bid, md.ask) {
            Some(quotes) => quotes,
            None => {
                // Cancel all orders if we can't quote profitably
                instructions.extend(self.cancel_all_orders());
                return instructions;
            }
        };

        let (our_bid, our_ask) = quotes;

        // Cancel existing orders (we'll replace them)
        instructions.extend(self.cancel_all_orders());

        // Place bid order (buying YES)
        if self.should_quote_side(Side::Yes) {
            let qty = self.calculate_quote_quantity(Side::Yes);
            if qty > 0 {
                let order_id = self.next_order_id();
                self.active_orders.push(order_id);
                
                instructions.push(OrderInstr::Limit {
                    side: Side::Yes,
                    price: our_bid,
                    qty,
                    ttl: self.params.order_ttl,
                });
            }
        }

        // Place ask order (buying NO, i.e., selling YES)
        if self.should_quote_side(Side::No) {
            let qty = self.calculate_quote_quantity(Side::No);
            if qty > 0 {
                let order_id = self.next_order_id();
                self.active_orders.push(order_id);
                
                instructions.push(OrderInstr::Limit {
                    side: Side::No,
                    price: our_ask,
                    qty,
                    ttl: self.params.order_ttl,
                });
            }
        }

        instructions
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Remove filled order from active orders
        self.active_orders.retain(|&id| id != fill.id);

        // Update position
        // Note: This is simplified - in practice you'd need to track which side each order was on
        // For now, we'll estimate based on price level
        // You could enhance this by maintaining a map of order_id -> side
        
        // For this simple implementation, we'll assume:
        // - Lower prices are YES buys (increase position)
        // - Higher prices are NO buys (decrease position)
        if fill.price < 50 {
            // Likely a YES buy
            self.position += fill.qty;
        } else {
            // Likely a NO buy
            self.position -= fill.qty;
        }
    }
}

/// A simple directional strategy that bets on one side
#[derive(Debug)]
pub struct DirectionalStrategy {
    target_side: Side,
    target_quantity: i64,
    max_price: u8,
    current_position: i64,
    order_ttl: Duration,
    order_id_counter: u64,
}

impl DirectionalStrategy {
    pub fn new(target_side: Side, target_quantity: i64, max_price: u8) -> Self {
        Self {
            target_side,
            target_quantity,
            max_price,
            current_position: 0,
            order_ttl: Duration::from_secs(10),
            order_id_counter: 0,
        }
    }

    fn next_order_id(&mut self) -> OrderId {
        self.order_id_counter += 1;
        self.order_id_counter
    }
}

impl KalshiStrategy for DirectionalStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        let mut instructions = Vec::new();

        // Check if we've reached our target position
        if self.current_position >= self.target_quantity {
            return instructions;
        }

        let remaining_qty = self.target_quantity - self.current_position;
        
        let (target_price, should_trade) = match self.target_side {
            Side::Yes => {
                // We want to buy YES
                let price = md.ask; // Buy at the ask
                (price, price <= self.max_price)
            }
            Side::No => {
                // We want to buy NO  
                let price = md.bid; // Buy at the bid
                (price, price <= self.max_price)
            }
        };

        if should_trade && remaining_qty > 0 {
            instructions.push(OrderInstr::Limit {
                side: self.target_side,
                price: target_price,
                qty: remaining_qty.min(10), // Limit order size
                ttl: self.order_ttl,
            });
        }

        instructions
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Update position based on the side we're targeting
        match self.target_side {
            Side::Yes => self.current_position += fill.qty,
            Side::No => self.current_position += fill.qty, // NO position is still positive quantity
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spread_mm_strategy() {
        let params = SpreadMmParams {
            min_spread: 4,
            post_qty: 100,
            max_pos: 500,
            order_ttl: Duration::from_secs(5),
        };

        let mut strategy = SpreadMmStrategy::new(params);

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 45,
            ask: 55, // 10 cent spread
        };

        let orders = strategy.on_book(&md);
        
        // Should generate 2 orders (bid and ask)
        assert_eq!(orders.len(), 2);

        // Check that orders are limit orders
        if let OrderInstr::Limit { side: Side::Yes, price, qty, .. } = &orders[0] {
            assert_eq!(*price, 46); // our_bid = market_bid + 1
            assert_eq!(*qty, 100);
        } else {
            panic!("Expected YES limit order");
        }

        if let OrderInstr::Limit { side: Side::No, price, qty, .. } = &orders[1] {
            assert_eq!(*price, 54); // our_ask = market_ask - 1  
            assert_eq!(*qty, 100);
        } else {
            panic!("Expected NO limit order");
        }
    }

    #[test]
    fn test_spread_too_narrow() {
        let params = SpreadMmParams {
            min_spread: 4,
            post_qty: 100,
            max_pos: 500,
            order_ttl: Duration::from_secs(5),
        };

        let mut strategy = SpreadMmStrategy::new(params);

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 48,
            ask: 50, // 2 cent spread (too narrow)
        };

        let orders = strategy.on_book(&md);
        
        // Should not generate any orders when spread is too narrow
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn test_directional_strategy() {
        let mut strategy = DirectionalStrategy::new(Side::Yes, 100, 60);

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 45,
            ask: 55,
        };

        let orders = strategy.on_book(&md);
        
        // Should generate 1 order to buy YES at the ask
        assert_eq!(orders.len(), 1);

        if let OrderInstr::Limit { side: Side::Yes, price, qty, .. } = &orders[0] {
            assert_eq!(*price, 55); // Buy at ask
            assert_eq!(*qty, 10);   // Limited order size
        } else {
            panic!("Expected YES limit order");
        }
    }
} 