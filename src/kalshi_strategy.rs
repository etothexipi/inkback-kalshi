use crate::kalshi_types::*;
use std::time::Duration;
use std::collections::HashMap;

/// Trait for Kalshi trading strategies
pub trait KalshiStrategy {
    /// Called when orderbook data is updated
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr>;
    
    /// Called when an order is filled
    fn on_fill(&mut self, fill: &Fill);
    
    /// Called when orders are created by the engine to provide real order IDs
    /// The vector contains (instruction_index, assigned_order_id) pairs
    fn on_orders_created(&mut self, order_mappings: Vec<(usize, OrderId)>);
    
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

    fn on_orders_created(&mut self, _order_mappings: Vec<(usize, OrderId)>) {
        // SpreadMmStrategy doesn't need to track individual order IDs for cancellation
        // since it uses TTL-based expiration rather than explicit cancellation
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

/// Parameters for the trailing market making strategy  
#[derive(Debug, Clone)]
pub struct TrailingMmParams {
    pub min_spread_for_trailing: u8,  // Only trail when spread >= this
    pub trail_distance: u8,           // How many cents outside best bid/ask
    pub max_position: i64,
    pub quote_quantity: i64,
    pub order_ttl: Duration,
    pub min_move_for_replace: u8,     // Cancel/replace if market moves this much
}

impl Default for TrailingMmParams {
    fn default() -> Self {
        Self {
            min_spread_for_trailing: 3,
            trail_distance: 2,
            max_position: 100,
            quote_quantity: 30,  // Smaller default size for this riskier strategy
            order_ttl: Duration::from_secs(3),  // Shorter TTL for quicker reactions
            min_move_for_replace: 1,  // Replace if bid/ask moves 1 cent
        }
    }
}

/// A trailing market making strategy that places orders outside the spread
/// to capture brief mispricings during volatile periods
#[derive(Debug)]
pub struct TrailingMmStrategy {
    params: TrailingMmParams,
    position: i64,
    active_orders: Vec<OrderId>,
    last_ticker: Option<String>,
    order_id_counter: u64,
    
    // Track last market state for order replacement decisions
    last_bid: Option<u8>,
    last_ask: Option<u8>,
    last_bid_order_price: Option<u8>,
    last_ask_order_price: Option<u8>,
    
    // Order tracking for proper cancellation
    pending_order_sides: Vec<Side>, // Track sides of orders we're waiting for IDs for
    
    // PnL tracking similar to SpreadMmStrategy
    total_pnl_cents: i64,
    order_side_map: HashMap<OrderId, Side>,
    avg_cost_basis_cents: f64,
    total_position_cost: i64,
}

impl TrailingMmStrategy {
    pub fn new(params: TrailingMmParams) -> Self {
        Self {
            params,
            position: 0,
            active_orders: Vec::new(),
            last_ticker: None,
            order_id_counter: 0,
            last_bid: None,
            last_ask: None,
            last_bid_order_price: None,
            last_ask_order_price: None,
            pending_order_sides: Vec::new(),
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
            let new_cost = fill_price as i64 * fill_qty;
            self.total_position_cost += new_cost;
            
            if self.position != 0 {
                self.avg_cost_basis_cents = self.total_position_cost as f64 / self.position.abs() as f64;
            }
        }
    }

    /// Calculate PnL when position reduces (round-trip)
    fn calculate_round_trip_pnl(&self, exit_price: u8, exit_qty: i64) -> i64 {
        let price_diff = if self.position > 0 {
            exit_price as i64 - self.avg_cost_basis_cents as i64
        } else {
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
        self.order_side_map.clear();
        self.pending_order_sides.clear();
        self.last_bid_order_price = None;
        self.last_ask_order_price = None;
        cancel_orders
    }

    /// Calculate trailing quote prices outside the spread
    fn calculate_trailing_quotes(&self, market_bid: u8, market_ask: u8) -> Option<(u8, u8)> {
        // Don't trail in uninitialized/fake conditions
        if market_bid == 0 || market_ask == 100 {
            return None;
        }

        let spread = market_ask.saturating_sub(market_bid);
        
        // Only trail when spread is large enough
        if spread < self.params.min_spread_for_trailing {
            return None;
        }

        // Calculate our trailing prices outside the spread
        let our_bid = market_bid.saturating_sub(self.params.trail_distance);
        let our_ask = market_ask.saturating_add(self.params.trail_distance);

        // Safety checks - don't place orders at extreme prices
        if our_bid <= 1 || our_ask >= 99 {
            return None;
        }

        // Make sure our bid is significantly below market bid
        if our_bid >= market_bid {
            return None;
        }

        // Make sure our ask is significantly above market ask  
        if our_ask <= market_ask {
            return None;
        }

        Some((our_bid, our_ask))
    }

    /// Check if market has moved enough to warrant order replacement
    fn should_replace_orders(&self, market_bid: u8, market_ask: u8) -> bool {
        // Replace if we don't have previous market data
        if self.last_bid.is_none() || self.last_ask.is_none() {
            return true;
        }
        
        let last_bid = self.last_bid.unwrap();
        let last_ask = self.last_ask.unwrap();
        
        // Replace if bid or ask moved by min_move_for_replace
        let bid_moved = market_bid.abs_diff(last_bid) >= self.params.min_move_for_replace;
        let ask_moved = market_ask.abs_diff(last_ask) >= self.params.min_move_for_replace;
        
        bid_moved || ask_moved
    }

    /// Determine whether to quote based on position limits
    fn should_quote_side(&self, side: Side) -> bool {
        match side {
            Side::Yes => self.position < self.params.max_position,
            Side::No => self.position > -self.params.max_position,
        }
    }

    /// Calculate quote quantity based on current position
    fn calculate_quote_quantity(&self, side: Side) -> i64 {
        let base_qty = self.params.quote_quantity;
        
        let qty = match side {
            Side::Yes => {
                let remaining_capacity = self.params.max_position - self.position;
                base_qty.min(remaining_capacity).max(0)
            }
            Side::No => {
                let remaining_capacity = self.position + self.params.max_position;
                base_qty.min(remaining_capacity).max(0)
            }
        };
        
        // Don't place orders that are too small to be efficient
        // Minimum quantity should be at least 5% of base quantity or 5, whichever is larger
        let min_qty = (base_qty / 20).max(5);
        if qty < min_qty {
            0
        } else {
            qty
        }
    }
}

impl KalshiStrategy for TrailingMmStrategy {
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        let mut instructions = Vec::new();

        // Cancel existing orders if ticker changed
        if self.last_ticker.as_ref() != Some(&md.ticker) {
            instructions.extend(self.cancel_all_orders());
            self.last_ticker = Some(md.ticker.clone());
        }

        // Check if we should replace orders due to market movement
        let should_replace = self.should_replace_orders(md.bid, md.ask);
        
        if should_replace && !self.active_orders.is_empty() {
            instructions.extend(self.cancel_all_orders());
        }

        // Calculate trailing quotes
        let quotes = match self.calculate_trailing_quotes(md.bid, md.ask) {
            Some(quotes) => quotes,
            None => {
                // Cancel all orders if we can't trail profitably
                instructions.extend(self.cancel_all_orders());
                self.last_bid = Some(md.bid);
                self.last_ask = Some(md.ask);
                return instructions;
            }
        };

        let (our_bid, our_ask) = quotes;

        // Only place new orders if we don't already have orders at these prices
        let bid_price_changed = self.last_bid_order_price != Some(our_bid);
        let ask_price_changed = self.last_ask_order_price != Some(our_ask);

        // Place trailing bid order (buying YES at discount to market)
        if self.should_quote_side(Side::Yes) && (bid_price_changed || self.active_orders.is_empty()) {
            let qty = self.calculate_quote_quantity(Side::Yes);
            if qty > 0 {
                // Track that we're placing a YES order (order ID will come from engine)
                self.pending_order_sides.push(Side::Yes);
                self.last_bid_order_price = Some(our_bid);
                
                instructions.push(OrderInstr::Limit {
                    side: Side::Yes,
                    price: our_bid,
                    qty,
                    ttl: self.params.order_ttl,
                });
            }
        }

        // Place trailing ask order (buying NO at discount to market, i.e., selling YES at premium)
        if self.should_quote_side(Side::No) && (ask_price_changed || self.active_orders.is_empty()) {
            let qty = self.calculate_quote_quantity(Side::No);
            if qty > 0 {
                // Track that we're placing a NO order (order ID will come from engine)
                self.pending_order_sides.push(Side::No);
                self.last_ask_order_price = Some(our_ask);
                
                instructions.push(OrderInstr::Limit {
                    side: Side::No,
                    price: our_ask,
                    qty,
                    ttl: self.params.order_ttl,
                });
            }
        }

        // Update last market state
        self.last_bid = Some(md.bid);
        self.last_ask = Some(md.ask);

        instructions
    }

    fn on_orders_created(&mut self, order_mappings: Vec<(usize, OrderId)>) {
        // Map instruction indices to real order IDs
        for (instruction_index, order_id) in order_mappings {
            if instruction_index < self.pending_order_sides.len() {
                let side = self.pending_order_sides[instruction_index];
                self.active_orders.push(order_id);
                self.order_side_map.insert(order_id, side);
            }
        }
        // Clear pending orders since they've been assigned IDs
        self.pending_order_sides.clear();
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Remove filled order from active orders
        self.active_orders.retain(|&id| id != fill.id);
        
        // Get the side this order was on
        let order_side = self.order_side_map.remove(&fill.id);
        
        // Calculate position change based on order side
        let (position_delta, is_position_increasing) = match order_side {
            Some(Side::Yes) => {
                let old_pos = self.position;
                (fill.qty, old_pos * fill.qty >= 0)
            },
            Some(Side::No) => {
                let old_pos = self.position;
                (-fill.qty, old_pos * (-fill.qty) >= 0)
            },
            None => {
                // Fallback logic
                if fill.price < 50 {
                    (fill.qty, self.position * fill.qty >= 0)
                } else {
                    (-fill.qty, self.position * (-fill.qty) >= 0)
                }
            }
        };
        
        // Calculate PnL if this is reducing our position
        if !is_position_increasing && self.position != 0 {
            let round_trip_pnl = self.calculate_round_trip_pnl(fill.price, position_delta.abs());
            self.total_pnl_cents += round_trip_pnl;
            
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

impl TrailingMmStrategy {
    /// Get the current PnL including unrealized PnL at given market price
    pub fn get_total_pnl_cents(&self, current_market_price: Option<u8>) -> i64 {
        let mut total = self.total_pnl_cents;
        
        if let Some(market_price) = current_market_price {
            if self.position != 0 {
                let unrealized_pnl = if self.position > 0 {
                    (market_price as i64 - self.avg_cost_basis_cents as i64) * self.position
                } else {
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
        
        let settlement_pnl = if self.position > 0 {
            (market_resolved_price as i64 - self.avg_cost_basis_cents as i64) * self.position
        } else {
            (self.avg_cost_basis_cents as i64 - market_resolved_price as i64) * self.position.abs()
        };
        
        self.total_pnl_cents += settlement_pnl;
        
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
    fn on_book(&mut self, md: &MarketData) -> Vec<OrderInstr> {
        // Check if we already have the target position
        let position_needed = match self.target_side {
            Side::Yes => self.target_quantity - self.current_position,
            Side::No => -self.target_quantity - self.current_position,
        };

        if position_needed <= 0 {
            return vec![];
        }

        // Determine the price we're willing to pay
        let target_price = match self.target_side {
            Side::Yes => md.ask.min(self.max_price),
            Side::No => md.bid.min(self.max_price),
        };

        // Don't place orders at extreme prices
        if target_price <= 1 || target_price >= 99 {
            return vec![];
        }

        // Limit order size to reasonable chunks
        let order_qty = position_needed.min(10);

        vec![OrderInstr::Limit {
            side: self.target_side,
            price: target_price,
            qty: order_qty,
            ttl: self.order_ttl,
        }]
    }

    fn on_orders_created(&mut self, _order_mappings: Vec<(usize, OrderId)>) {
        // DirectionalStrategy doesn't need to track order IDs since it doesn't cancel orders
    }

    fn on_fill(&mut self, fill: &Fill) {
        // Update current position based on fill
        // Use price-based heuristic since Fill doesn't have side info
        if fill.price < 50 {
            // Likely a YES buy (or NO sell)
            self.current_position += fill.qty;
        } else {
            // Likely a NO buy (or YES sell)  
            self.current_position -= fill.qty;
        }
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

    #[test]
    fn test_trailing_mm_strategy_basic() {
        let params = TrailingMmParams {
            min_spread_for_trailing: 3,
            trail_distance: 2,
            max_position: 100,
            quote_quantity: 30,
            order_ttl: Duration::from_secs(3),
            min_move_for_replace: 1,
        };

        let mut strategy = TrailingMmStrategy::new(params);

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 40,
            ask: 50, // 10 cent spread (>= 3, so should trail)
        };

        let orders = strategy.on_book(&md);
        
        // Should generate 2 trailing orders
        assert_eq!(orders.len(), 2);

        // Check trailing bid order (40 - 2 = 38)
        if let OrderInstr::Limit { side: Side::Yes, price, qty, .. } = &orders[0] {
            assert_eq!(*price, 38); // bid - trail_distance
            assert_eq!(*qty, 30);
        } else {
            panic!("Expected YES trailing order");
        }

        // Check trailing ask order (50 + 2 = 52)  
        if let OrderInstr::Limit { side: Side::No, price, qty, .. } = &orders[1] {
            assert_eq!(*price, 52); // ask + trail_distance
            assert_eq!(*qty, 30);
        } else {
            panic!("Expected NO trailing order");
        }
    }

    #[test]
    fn test_trailing_mm_spread_too_narrow() {
        let params = TrailingMmParams {
            min_spread_for_trailing: 5,
            trail_distance: 2,
            max_position: 100,
            quote_quantity: 30,
            order_ttl: Duration::from_secs(3),
            min_move_for_replace: 1,
        };

        let mut strategy = TrailingMmStrategy::new(params);

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 48,
            ask: 52, // 4 cent spread (< 5, so shouldn't trail)
        };

        let orders = strategy.on_book(&md);
        
        // Should not generate any orders when spread is too narrow
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn test_trailing_mm_order_replacement() {
        let params = TrailingMmParams {
            min_spread_for_trailing: 3,
            trail_distance: 2,
            max_position: 100,
            quote_quantity: 30,
            order_ttl: Duration::from_secs(3),
            min_move_for_replace: 1,
        };

        let mut strategy = TrailingMmStrategy::new(params);

        // First market update
        let md1 = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 40,
            ask: 50,
        };

        let orders1 = strategy.on_book(&md1);
        assert_eq!(orders1.len(), 2); // Initial orders

        // Market moves by 1 cent (should trigger replacement)
        let md2 = MarketData {
            ts: 2000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 41, // moved up 1 cent
            ask: 51, // moved up 1 cent
        };

        let orders2 = strategy.on_book(&md2);
        
        // Should generate cancel orders + new orders
        let cancel_count = orders2.iter().filter(|o| matches!(o, OrderInstr::Cancel { .. })).count();
        let limit_count = orders2.iter().filter(|o| matches!(o, OrderInstr::Limit { .. })).count();
        
        assert_eq!(cancel_count, 2); // Cancel previous 2 orders
        assert_eq!(limit_count, 2);  // Place 2 new orders
    }

    #[test]
    fn test_trailing_mm_extreme_prices() {
        let params = TrailingMmParams {
            min_spread_for_trailing: 3,
            trail_distance: 2,
            max_position: 100,
            quote_quantity: 30,
            order_ttl: Duration::from_secs(3),
            min_move_for_replace: 1,
        };

        let mut strategy = TrailingMmStrategy::new(params);

        // Test near price extremes
        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 2,  // Very low bid
            ask: 6,  // Would create trailing bid at 0, ask at 8
        };

        let orders = strategy.on_book(&md);
        
        // Should not place orders when trailing would hit extreme prices
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn test_trailing_mm_position_limits() {
        let params = TrailingMmParams {
            min_spread_for_trailing: 3,
            trail_distance: 2,
            max_position: 10, // Low limit to test constraint
            quote_quantity: 30,
            order_ttl: Duration::from_secs(3),
            min_move_for_replace: 1,
        };

        let mut strategy = TrailingMmStrategy::new(params);
        
        // Set position near limit
        strategy.position = 9; // Close to max_position = 10

        let md = MarketData {
            ts: 1000,
            ticker: "TEST-CONTRACT".to_string(),
            bid: 40,
            ask: 50,
        };

        let orders = strategy.on_book(&md);
        
        // Should only place NO order (can't buy more YES due to position limit)
        assert_eq!(orders.len(), 1);
        
        if let OrderInstr::Limit { side: Side::No, .. } = &orders[0] {
            // Good - only NO order placed
        } else {
            panic!("Expected only NO order due to position limits");
        }
    }
} 