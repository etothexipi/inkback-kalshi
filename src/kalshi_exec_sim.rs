use crate::kalshi_types::{
    ActiveOrder, Event, Fill, MarketData, Metrics, OrderAudit, OrderId, OrderInstr, 
    OrderStatus, PendingOrder, Report, Side, SimConfig
};
use crate::kalshi_strategy::KalshiStrategy;
use crate::kalshi_l2_book::{OrderBook, update_book_with_event};
use anyhow::Result;
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;
use std::time::Duration;

/// Main execution simulator engine
#[derive(Debug)]
pub struct Engine {
    state: SimState,
    config: SimConfig,
    event_count: usize, // For debug output
}

/// Internal simulation state
#[derive(Debug)]
struct SimState {
    books: HashMap<String, OrderBook>,
    pending: BinaryHeap<Reverse<PendingOrderWrapper>>,
    active: HashMap<OrderId, ActiveOrder>,
    latency: Duration,
    metrics: Metrics,
    order_audit: Vec<OrderAudit>,
    order_id_counter: OrderId,
    position_tracker: HashMap<OrderId, Side>, // Track which side each order is on
}

/// Wrapper for pending orders to enable ordering in the heap
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingOrderWrapper {
    order: PendingOrder,
}

impl PartialOrd for PendingOrderWrapper {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingOrderWrapper {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Earlier activation times have higher priority (min-heap)
        self.order.activation_time.cmp(&other.order.activation_time)
    }
}

impl Engine {
    /// Create a new execution engine
    pub fn new(config: SimConfig) -> Self {
        Self {
            state: SimState {
                books: HashMap::new(),
                pending: BinaryHeap::new(),
                active: HashMap::new(),
                latency: config.latency,
                metrics: Metrics::default(),
                order_audit: Vec::new(),
                order_id_counter: 0,
                position_tracker: HashMap::new(),
            },
            config,
            event_count: 0,
        }
    }

    /// Run the simulation with the given strategy and events
    pub fn run<I>(
        &mut self,
        strategy: &mut dyn KalshiStrategy,
        events: I,
    ) -> Result<Report>
    where
        I: Iterator<Item = Result<Event>>,
    {
        let mut event_count = 0;
        let mut delta_count = 0;

        for event_result in events {
            let event = event_result?;
            event_count += 1;
            
            // Check if we should stop based on config
            if let Some(end_time) = self.config.end_time {
                if event.ts() > end_time {
                    break;
                }
            }

            // Skip events before start time
            if event.ts() < self.config.start_time {
                continue;
            }

            // Track delta events for debugging
            if matches!(event, Event::Delta { .. }) {
                delta_count += 1;
            }

            self.process_event(event, strategy)?;
        }

        // Finalize metrics and settle remaining positions
        self.finalize_strategy_pnl(strategy);
        self.state.metrics.calculate_win_rate();

        Ok(Report {
            metrics: self.state.metrics.clone(),
            fills: self.extract_fills_from_audit(),
            orders: self.state.order_audit.clone(),
        })
    }
    
    /// Finalize PnL by settling any remaining positions based on market resolution
    fn finalize_strategy_pnl(&mut self, strategy: &mut dyn KalshiStrategy) {
        // For SpreadMmStrategy, we need to settle final positions
        if let Some(spread_strategy) = strategy.as_any().downcast_mut::<crate::kalshi_strategy::SpreadMmStrategy>() {
            // Determine market resolution from final orderbook states
            for (ticker, book) in &self.state.books {
                let market_resolved_price = self.determine_market_resolution(book);
                let settlement_pnl = spread_strategy.settle_final_position(market_resolved_price);
                
                // Add settlement PnL to metrics
                self.state.metrics.gross_pnl_cents += settlement_pnl;
            }
            
            // Get total PnL from strategy (including all round-trips)
            let total_strategy_pnl = spread_strategy.get_total_pnl_cents(None);
            
            // Update metrics with the correct total
            self.state.metrics.gross_pnl_cents = total_strategy_pnl;
        }
    }
    
    /// Determine market resolution based on final orderbook state
    fn determine_market_resolution(&self, book: &crate::kalshi_l2_book::OrderBook) -> u8 {
        let (bid, ask) = book.best_bid_ask();
        
        // Market resolution logic:
        // - If bids are at 99¢ and no meaningful asks, YES wins (100¢)
        // - If asks are at 1¢ and no meaningful bids, NO wins (0¢)
        // - Otherwise, use mid-price as settlement
        
        if bid >= 99 && ask >= 100 {
            // YES clearly winning
            100
        } else if ask <= 1 && bid == 0 {
            // NO clearly winning  
            0
        } else if bid > 0 && ask < 100 {
            // Use mid-price for settlement
            (bid + ask) / 2
        } else {
            // Default to 50¢ if unclear
            50
        }
    }

    fn process_event(&mut self, event: Event, strategy: &mut dyn KalshiStrategy) -> Result<()> {
        let event_ts = event.ts();
        let ticker = event.ticker().to_string();

        // Step 1: Activate pending orders
        self.activate_pending_orders(event_ts);

        // Step 2: Apply market event to orderbook
        let book = self.state.books.entry(ticker.clone()).or_insert_with(OrderBook::new);
        update_book_with_event(book, &event)?;

        self.event_count += 1;

        // Step 3: Process fills if this is a trade event
        if let Event::Trade { yes_price, no_price, qty, taker, .. } = &event {
            self.process_trade_fills(*yes_price, *no_price, *qty, *taker, event_ts, strategy);
        }

        // Step 4: Feed strategy with updated market data
        // Get the bid/ask after the event is processed
        let (bid, ask) = self.state.books.get(&ticker)
            .map(|book| book.best_bid_ask())
            .unwrap_or((0, 100));
        
        let market_data = MarketData {
            ts: event_ts,
            ticker: ticker.clone(),
            bid,
            ask,
        };

        let instructions = strategy.on_book(&market_data);
        let order_mappings = self.process_order_instructions(instructions, event_ts)?;
        
        // Notify strategy of assigned order IDs
        if !order_mappings.is_empty() {
            strategy.on_orders_created(order_mappings);
        }

        // Step 5: Expire orders
        self.expire_orders(event_ts, strategy);

        Ok(())
    }

    fn activate_pending_orders(&mut self, current_ts: u64) {
        let mut to_activate = Vec::new();

        // Extract orders ready for activation
        while let Some(Reverse(wrapper)) = self.state.pending.peek() {
            if wrapper.order.activation_time <= current_ts {
                let wrapper = self.state.pending.pop().unwrap().0;
                to_activate.push(wrapper.order);
            } else {
                break;
            }
        }

        // Activate orders
        for pending_order in to_activate {
            let active_order = pending_order.into_active();
            let order_id = active_order.id;
            
            // Update audit trail
            if let Some(audit) = self.state.order_audit.iter_mut().find(|a| a.id == order_id) {
                audit.status = OrderStatus::Active;
            }

            self.state.active.insert(order_id, active_order);
        }
    }

    fn process_trade_fills(
        &mut self,
        yes_price: u8,
        no_price: u8,
        mut qty_remaining: i64,
        taker_side: Side,
        ts: u64,
        strategy: &mut dyn KalshiStrategy,
    ) {
        // Process fills for both sides based on the trade
        match taker_side {
            Side::Yes => {
                // YES taker means someone bought YES at yes_price
                // This can fill NO orders (sell YES) at or below yes_price
                self.process_side_fills(Side::No, yes_price, &mut qty_remaining, ts, strategy, true);
            }
            Side::No => {
                // NO taker means someone bought NO at no_price  
                // This can fill YES orders (buy YES) at or above (100 - no_price)
                let effective_yes_price = 100 - no_price;
                self.process_side_fills(Side::Yes, effective_yes_price, &mut qty_remaining, ts, strategy, false);
            }
        }
    }

    fn process_side_fills(
        &mut self,
        fill_side: Side,
        trade_price: u8,
        qty_remaining: &mut i64,
        ts: u64,
        strategy: &mut dyn KalshiStrategy,
        is_at_or_below: bool, // true for at-or-below, false for at-or-above
    ) {
        // Find all active orders that can be filled
        let mut orders_to_fill: Vec<_> = self.state.active
            .iter()
            .filter(|(_, order)| {
                if order.side != fill_side {
                    return false;
                }
                
                // Check if order price allows for a fill at trade_price
                if is_at_or_below {
                    // For NO orders: fill if order price >= trade_price (selling at higher price is better)
                    order.price >= trade_price
                } else {
                    // For YES orders: fill if order price <= trade_price (buying at lower price is better)
                    order.price <= trade_price
                }
            })
            .map(|(&id, order)| (id, order.clone()))
            .collect();

        // Sort by price priority (best prices first) and then by order ID for deterministic fills
        if is_at_or_below {
            // For NO orders, prioritize lower prices (better for the seller)
            orders_to_fill.sort_by_key(|(id, order)| (order.price, *id));
        } else {
            // For YES orders, prioritize higher prices (better for the buyer) 
            orders_to_fill.sort_by_key(|(id, order)| (std::cmp::Reverse(order.price), *id));
        }

        for (order_id, mut order) in orders_to_fill {
            if *qty_remaining <= 0 {
                break;
            }

            let fill_qty = (*qty_remaining).min(order.remaining);
            
            // Fill at the order's limit price (price improvement for the order placer)
            let fill_price = order.price;
            
            // Create fill
            let fill = Fill {
                id: order_id,
                price: fill_price,
                qty: fill_qty,
                ts,
            };

            // Update metrics
            let is_buy = order.side == Side::Yes;
            self.state.metrics.update_fill(&fill, is_buy, fill_price as i64);

            // Notify strategy
            strategy.on_fill(&fill);

            // Update order
            order.remaining -= fill_qty;
            *qty_remaining -= fill_qty;

            if order.remaining <= 0 {
                // Order fully filled
                self.state.active.remove(&order_id);
                self.state.position_tracker.remove(&order_id);
                
                // Update audit trail
                if let Some(audit) = self.state.order_audit.iter_mut().find(|a| a.id == order_id) {
                    audit.status = OrderStatus::Filled;
                    audit.filled_qty = audit.original_qty;
                }
            } else {
                // Partially filled
                self.state.active.insert(order_id, order);
                
                // Update audit trail
                if let Some(audit) = self.state.order_audit.iter_mut().find(|a| a.id == order_id) {
                    audit.status = OrderStatus::PartiallyFilled;
                    audit.filled_qty += fill_qty;
                }
            }
        }
    }

    fn process_order_instructions(
        &mut self,
        instructions: Vec<OrderInstr>,
        current_ts: u64,
    ) -> Result<Vec<(usize, OrderId)>> {
        let mut order_mappings = Vec::new();
        
        for (instruction_index, instruction) in instructions.into_iter().enumerate() {
            match instruction {
                OrderInstr::Limit { side, price, qty, ttl } => {
                    let order_id = self.next_order_id();
                    let activation_time = current_ts + self.state.latency.as_millis() as u64;
                    let expiry_time = activation_time + ttl.as_millis() as u64;

                    let pending_order = PendingOrder {
                        id: order_id,
                        side,
                        price,
                        qty,
                        activation_time,
                        expiry_time,
                    };

                    // Add to pending queue
                    self.state.pending.push(Reverse(PendingOrderWrapper {
                        order: pending_order.clone(),
                    }));

                    // Track order side
                    self.state.position_tracker.insert(order_id, side);

                    // Create audit trail
                    let audit = OrderAudit {
                        id: order_id,
                        side,
                        price,
                        original_qty: qty,
                        filled_qty: 0,
                        create_time: current_ts,
                        activation_time,
                        expiry_time,
                        status: OrderStatus::Created,
                    };

                    self.state.order_audit.push(audit);
                    self.state.metrics.total_orders += 1;
                    
                    // Track this order ID mapping
                    order_mappings.push((instruction_index, order_id));
                }
                OrderInstr::Cancel { id } => {
                    // Remove from active orders
                    if self.state.active.remove(&id).is_some() {
                        self.state.position_tracker.remove(&id);
                        self.state.metrics.cancelled_orders += 1;

                        // Update audit trail
                        if let Some(audit) = self.state.order_audit.iter_mut().find(|a| a.id == id) {
                            audit.status = OrderStatus::Cancelled;
                        }
                    }
                }
            }
        }
        Ok(order_mappings)
    }

    fn expire_orders(&mut self, current_ts: u64, _strategy: &mut dyn KalshiStrategy) {
        let mut expired_orders = Vec::new();

        // Find expired active orders
        for (&order_id, order) in &self.state.active {
            if order.expiry_time <= current_ts {
                expired_orders.push(order_id);
            }
        }

        // Remove expired orders
        for order_id in expired_orders {
            self.state.active.remove(&order_id);
            self.state.position_tracker.remove(&order_id);
            self.state.metrics.expired_orders += 1;

            // Update audit trail
            if let Some(audit) = self.state.order_audit.iter_mut().find(|a| a.id == order_id) {
                audit.status = OrderStatus::Expired;
            }

            // Note: We could notify strategy about expirations if needed
        }
    }

    fn next_order_id(&mut self) -> OrderId {
        self.state.order_id_counter += 1;
        self.state.order_id_counter
    }

    fn extract_fills_from_audit(&self) -> Vec<Fill> {
        // In a real implementation, you'd track fills separately
        // For now, we'll derive them from the audit trail
        self.state.order_audit
            .iter()
            .filter(|audit| audit.filled_qty > 0)
            .map(|audit| Fill {
                id: audit.id,
                price: audit.price,
                qty: audit.filled_qty,
                ts: audit.activation_time, // Approximation
            })
            .collect()
    }

    /// Get current active orders count
    pub fn active_orders_count(&self) -> usize {
        self.state.active.len()
    }

    /// Get current metrics snapshot
    pub fn get_metrics(&self) -> &Metrics {
        &self.state.metrics
    }

    /// Get orderbook for a ticker
    pub fn get_book(&self, ticker: &str) -> Option<&OrderBook> {
        self.state.books.get(ticker)
    }
}

/// Convenience function to run a backtest
pub fn run_backtest<I>(
    config: SimConfig,
    strategy: &mut dyn KalshiStrategy,
    events: I,
) -> Result<Report>
where
    I: Iterator<Item = Result<Event>>,
{
    let mut engine = Engine::new(config);
    engine.run(strategy, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kalshi_types::{Event, Side};
    use crate::kalshi_strategy::{SpreadMmStrategy, SpreadMmParams};
    use std::time::Duration;

    #[test]
    fn test_engine_basic_flow() {
        let config = SimConfig {
            latency: Duration::from_millis(1),
            start_time: 0,
            end_time: Some(10000),
        };

        let mut strategy = SpreadMmStrategy::new(SpreadMmParams::default());

        let events = vec![
            Ok(Event::Snapshot {
                seq: 1,
                ts: 1000,
                ticker: "TEST".to_string(),
                yes_levels: vec![(45, 100)],
                no_levels: vec![(55, 100)],
            }),
            Ok(Event::Trade {
                seq: 2,
                ts: 2000,
                ticker: "TEST".to_string(),
                yes_price: 46,
                no_price: 54,
                qty: 10,
                taker: Side::Yes,
            }),
        ];

        let result = run_backtest(config, &mut strategy, events.into_iter()).unwrap();
        
        assert!(result.metrics.total_orders > 0);
        assert_eq!(result.orders.len(), result.metrics.total_orders);
    }

    #[test]
    fn test_order_activation_timing() {
        let config = SimConfig {
            latency: Duration::from_millis(100),
            start_time: 0,
            end_time: None,
        };

        let mut engine = Engine::new(config);

        // Create a strategy that places one order
        struct TestStrategy;
        impl KalshiStrategy for TestStrategy {
            fn on_book(&mut self, _md: &MarketData) -> Vec<OrderInstr> {
                vec![OrderInstr::Limit {
                    side: Side::Yes,
                    price: 45,
                    qty: 100,
                    ttl: Duration::from_secs(5),
                }]
            }
            fn on_fill(&mut self, _fill: &Fill) {}
            fn on_orders_created(&mut self, _order_mappings: Vec<(usize, OrderId)>) {}
            fn as_any(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut strategy = TestStrategy;

        let events = vec![
            Ok(Event::Snapshot {
                seq: 1,
                ts: 1000,
                ticker: "TEST".to_string(),
                yes_levels: vec![(45, 100)],
                no_levels: vec![(55, 100)],
            }),
        ];

        let _result = engine.run(&mut strategy, events.into_iter()).unwrap();
        
        // Order should be created but not yet active due to latency
        assert_eq!(engine.active_orders_count(), 0);
    }

    #[test]
    fn test_fill_logic() {
        let config = SimConfig {
            latency: Duration::from_millis(0), // No latency for this test
            start_time: 0,
            end_time: None,
        };

        let mut engine = Engine::new(config);

        struct TestStrategy {
            order_placed: bool,
        }
        impl KalshiStrategy for TestStrategy {
            fn on_book(&mut self, _md: &MarketData) -> Vec<OrderInstr> {
                if !self.order_placed {
                    self.order_placed = true;
                    vec![OrderInstr::Limit {
                        side: Side::No,
                        price: 55,
                        qty: 50,
                        ttl: Duration::from_secs(10),
                    }]
                } else {
                    vec![]
                }
            }
            fn on_fill(&mut self, fill: &Fill) {
                println!("Received fill: {:?}", fill);
            }
            fn on_orders_created(&mut self, _order_mappings: Vec<(usize, OrderId)>) {}
            fn as_any(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut strategy = TestStrategy { order_placed: false };

        let events = vec![
            Ok(Event::Snapshot {
                seq: 1,
                ts: 1000,
                ticker: "TEST".to_string(),
                yes_levels: vec![(45, 100)],
                no_levels: vec![(55, 100)],
            }),
            Ok(Event::Trade {
                seq: 2,
                ts: 2000,
                ticker: "TEST".to_string(),
                yes_price: 55,
                no_price: 55,
                qty: 25,
                taker: Side::Yes, // YES taker should fill NO orders
            }),
        ];

        let result = engine.run(&mut strategy, events.into_iter()).unwrap();
        
        // Should have some fills
        assert!(result.metrics.volume_traded > 0);
    }
} 