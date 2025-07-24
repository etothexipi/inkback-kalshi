use crate::kalshi_types::*;
use std::time::Duration;
use std::collections::HashMap;

/// Trait for Kalshi trading strategies
pub trait KalshiStrategy {
    /// Called when orderbook data is updated
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr>;
    
    /// Called when an order is filled
    fn on_fill(&mut self, fill: &Fill);
    
    /// Support for downcasting to concrete strategy types
    fn as_any(&mut self) -> &mut dyn std::any::Any;
}

/// Parameters for the spread market making strategy
#[derive(Debug, Clone)]
pub struct SpreadMmParams {
    pub min_spread: u8,
    pub max_position: i64,
    pub quote_quantity: i64,
    pub order_ttl: Duration,
}

impl Default for SpreadMmParams {
    fn default() -> Self {
        Self {
            min_spread: 4,
            max_position: 100,
            quote_quantity: 50,
            order_ttl: Duration::from_secs(5),
        }
    }
}

/// A spread-based market making strategy
#[derive(Debug)]
pub struct SpreadMmStrategy {
    params: SpreadMmParams,
    position: i64,  // Net YES position (+ve = long YES, -ve = short YES)
    active_orders: Vec<OrderId>,
    last_ticker: Option<String>,
    order_id_counter: u64,
    
    // Enhanced PnL tracking for market making
    total_pnl_cents: i64,              // Running total of round-trip PnL
    order_side_map: HashMap<OrderId, Side>, // Track which side each order is on
    avg_cost_basis_cents: f64,         // For final settlement calculation
    total_position_cost: i64,          // Total cost of current position
}

impl SpreadMmStrategy {
    pub fn new(params: SpreadMmParams) -> Self {
        Self {
            params,
            position: 0,
            active_orders: Vec::new(),
            last_ticker: None,
            order_id_counter: 0,
            total_pnl_cents: 0,
            order_side_map: HashMap::new(),
            avg_cost_basis_cents: 0.0,
            total_position_cost: 0,
        }
    }

    fn next_order_id(&mut self) -> OrderId {
        self.order_id_counter += 1;
        self.order_id_counter
    }

    /// Calculate running average cost basis for position
    fn update_cost_basis(&mut self, fill_price: u8, fill_qty: i64, is_position_increasing: bool) {
        if is_position_increasing {
            // Position is growing, update cost basis
            let new_cost = fill_price as i64 * fill_qty;
            self.total_position_cost += new_cost;
            
            if self.position != 0 {
                self.avg_cost_basis_cents = self.total_position_cost as f64 / self.position.abs() as f64;
            }
        }
    }

    /// Calculate PnL when position reduces (round-trip)
    fn calculate_round_trip_pnl(&self, exit_price: u8, exit_qty: i64) -> i64 {
        // For market making, we make money on the spread
        // If we're reducing a long position, we're selling at exit_price vs avg_cost_basis
        // If we're reducing a short position, we're buying at exit_price vs avg_cost_basis
        
        let price_diff = if self.position > 0 {
            // Long position, selling: PnL = (exit_price - cost_basis) * qty
            exit_price as i64 - self.avg_cost_basis_cents as i64
        } else {
            // Short position, buying: PnL = (cost_basis - exit_price) * qty  
            self.avg_cost_basis_cents as i64 - exit_price as i64
        };
        
        price_diff * exit_qty
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
                self.position < self.params.max_position
            }
            Side::No => {
                // Can buy NO (go short YES) if position > -max_pos
                self.position > -self.params.max_position
            }
        }
    }

    /// Adjust quantities based on current position
    fn calculate_quote_quantity(&self, side: Side) -> i64 {
        let base_qty = self.params.quote_quantity;
        
        match side {
            Side::Yes => {
                // Buying YES: reduce quantity as we get more long
                let remaining_capacity = self.params.max_position - self.position;
                base_qty.min(remaining_capacity).max(0)
            }
            Side::No => {
                // Buying NO (shorting YES): reduce quantity as we get more short
                let remaining_capacity = self.position + self.params.max_position;
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

        // Place bid order (buying YES)
        if self.should_quote_side(Side::Yes) {
            let qty = self.calculate_quote_quantity(Side::Yes);
            if qty > 0 {
                let order_id = self.next_order_id();
                self.active_orders.push(order_id);
                
                // Track that this order is a YES bid
                self.order_side_map.insert(order_id, Side::Yes);
                
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
                
                // Track that this order is a NO bid (equivalent to YES ask)
                self.order_side_map.insert(order_id, Side::No);
                
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
        
        // Get the side this order was on
        let order_side = self.order_side_map.remove(&fill.id);
        
        // Calculate position change based on order side
        let (position_delta, is_position_increasing) = match order_side {
            Some(Side::Yes) => {
                // YES buy order filled - increases YES position
                let old_pos = self.position;
                (fill.qty, old_pos * fill.qty >= 0) // increasing if same sign or was zero
            },
            Some(Side::No) => {
                // NO buy order filled - decreases YES position (equivalent to selling YES)
                let old_pos = self.position;
                (-fill.qty, old_pos * (-fill.qty) >= 0) // increasing if same sign or was zero
            },
            None => {
                // Fallback to old logic if we don't have order side info
                if fill.price < 50 {
                    (fill.qty, self.position * fill.qty >= 0)
                } else {
                    (-fill.qty, self.position * (-fill.qty) >= 0)
                }
            }
        };
        
        // Calculate PnL if this is reducing our position (round-trip)
        if !is_position_increasing && self.position != 0 {
            let round_trip_pnl = self.calculate_round_trip_pnl(fill.price, position_delta.abs());
            self.total_pnl_cents += round_trip_pnl;
            
            // Update cost basis by removing the portion we just closed
            let closed_qty = position_delta.abs();
            self.total_position_cost -= self.avg_cost_basis_cents as i64 * closed_qty;
        }
        
        // Update position
        self.position += position_delta;
        
        // Update cost basis if position is increasing
        if is_position_increasing {
            self.update_cost_basis(fill.price, position_delta.abs(), true);
        }
        
        // Recalculate cost basis if we still have position
        if self.position != 0 && self.total_position_cost > 0 {
            self.avg_cost_basis_cents = self.total_position_cost as f64 / self.position.abs() as f64;
        } else if self.position == 0 {
            self.avg_cost_basis_cents = 0.0;
            self.total_position_cost = 0;
        }
    }
    
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl SpreadMmStrategy {
    /// Get the current PnL including unrealized PnL at given market price
    pub fn get_total_pnl_cents(&self, current_market_price: Option<u8>) -> i64 {
        let mut total = self.total_pnl_cents;
        
        // Add unrealized PnL for current position
        if let Some(market_price) = current_market_price {
            if self.position != 0 {
                let unrealized_pnl = if self.position > 0 {
                    // Long position: PnL = (current_price - cost_basis) * position
                    (market_price as i64 - self.avg_cost_basis_cents as i64) * self.position
                } else {
                    // Short position: PnL = (cost_basis - current_price) * abs(position)
                    (self.avg_cost_basis_cents as i64 - market_price as i64) * self.position.abs()
                };
                total += unrealized_pnl;
            }
        }
        
        total
    }
    
    /// Settle final position based on market resolution
    pub fn settle_final_position(&mut self, market_resolved_price: u8) -> i64 {
        if self.position == 0 {
            return 0;
        }
        
        // Calculate final settlement PnL
        let settlement_pnl = if self.position > 0 {
            // Long YES position: get (resolved_price - cost_basis) * position
            (market_resolved_price as i64 - self.avg_cost_basis_cents as i64) * self.position
        } else {
            // Short YES position: get (cost_basis - resolved_price) * abs(position)
            (self.avg_cost_basis_cents as i64 - market_resolved_price as i64) * self.position.abs()
        };
        
        self.total_pnl_cents += settlement_pnl;
        
        // Clear position
        self.position = 0;
        self.avg_cost_basis_cents = 0.0;
        self.total_position_cost = 0;
        
        settlement_pnl
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
    fn on_book(&mut self, _md: &MarketData) -> Vec<OrderInstr> {
        // Simple directional strategy implementation
        vec![]
    }

    fn on_fill(&mut self, _fill: &Fill) {
        // Handle fills for directional strategy
    }
    
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spread_mm_strategy() {
        let params = SpreadMmParams {
            min_spread: 4,
            max_position: 100,
            quote_quantity: 50,
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
            assert_eq!(*qty, 50);
        } else {
            panic!("Expected YES limit order");
        }

        if let OrderInstr::Limit { side: Side::No, price, qty, .. } = &orders[1] {
            assert_eq!(*price, 54); // our_ask = market_ask - 1  
            assert_eq!(*qty, 50);
        } else {
            panic!("Expected NO limit order");
        }
    }

    #[test]
    fn test_spread_too_narrow() {
        let params = SpreadMmParams {
            min_spread: 4,
            max_position: 100,
            quote_quantity: 50,
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