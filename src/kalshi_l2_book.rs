use crate::kalshi_types::{Event, Side};
use anyhow::Result;
use std::collections::BTreeMap;

/// L2 order book for Kalshi YES/NO contracts
/// YES levels are treated as bids, NO levels are treated as asks
#[derive(Debug, Clone)]
pub struct OrderBook {
    /// YES side levels (price -> quantity), sorted ascending
    yes_levels: BTreeMap<u8, i64>,
    /// NO side levels (price -> quantity), sorted ascending  
    no_levels: BTreeMap<u8, i64>,
    /// Last update timestamp
    last_update_ts: u64,
}

impl OrderBook {
    /// Create a new empty orderbook
    pub fn new() -> Self {
        Self {
            yes_levels: BTreeMap::new(),
            no_levels: BTreeMap::new(),
            last_update_ts: 0,
        }
    }

    /// Apply a snapshot event to reset the orderbook
    pub fn apply_snapshot(&mut self, yes_levels: Vec<(u8, i64)>, no_levels: Vec<(u8, i64)>, ts: u64) -> Result<()> {
        // Clear existing levels
        self.yes_levels.clear();
        self.no_levels.clear();

        // Add YES levels
        for (price, qty) in yes_levels {
            if qty > 0 {
                self.yes_levels.insert(price, qty);
            }
        }

        // Add NO levels
        for (price, qty) in no_levels {
            if qty > 0 {
                self.no_levels.insert(price, qty);
            }
        }

        self.last_update_ts = ts;
        self.validate_book()?;

        Ok(())
    }

    /// Apply a delta event to update a single price level
    pub fn apply_delta(&mut self, side: Side, price: u8, delta: i64, ts: u64) -> Result<()> {
        let levels = match side {
            Side::Yes => &mut self.yes_levels,
            Side::No => &mut self.no_levels,
        };

        // Get current quantity at this price level
        let current_qty = levels.get(&price).copied().unwrap_or(0);
        let new_qty = current_qty + delta;

        if new_qty <= 0 {
            // Remove the level if quantity becomes zero or negative
            levels.remove(&price);
        } else {
            // Update the level with new quantity
            levels.insert(price, new_qty);
        }

        self.last_update_ts = ts;
        self.validate_book()?;

        Ok(())
    }

    /// Get the best bid and ask prices in cents
    /// Returns (bid_price, ask_price) where:
    /// - bid_price is the highest YES price (best price to sell YES)
    /// - ask_price is the lowest NO price (best price to buy YES)
    pub fn best_bid_ask(&self) -> (u8, u8) {
        let best_bid = self.yes_levels
            .iter()
            .rev() // Highest price first
            .next()
            .map(|(&price, _)| price)
            .unwrap_or(0);

        let best_ask = self.no_levels
            .iter()
            .next() // Lowest price first
            .map(|(&price, _)| price)
            .unwrap_or(100); // Default to 100 cents if no asks

        (best_bid, best_ask)
    }

    /// Get the best bid price and quantity
    pub fn best_bid(&self) -> Option<(u8, i64)> {
        self.yes_levels
            .iter()
            .rev()
            .next()
            .map(|(&price, &qty)| (price, qty))
    }

    /// Get the best ask price and quantity
    pub fn best_ask(&self) -> Option<(u8, i64)> {
        self.no_levels
            .iter()
            .next()
            .map(|(&price, &qty)| (price, qty))
    }

    /// Get quantity available at a specific price and side
    pub fn get_quantity(&self, side: Side, price: u8) -> i64 {
        match side {
            Side::Yes => self.yes_levels.get(&price).copied().unwrap_or(0),
            Side::No => self.no_levels.get(&price).copied().unwrap_or(0),
        }
    }

    /// Get all YES levels (bids) as a vector sorted by price descending
    pub fn get_yes_levels(&self) -> Vec<(u8, i64)> {
        self.yes_levels
            .iter()
            .rev()
            .map(|(&price, &qty)| (price, qty))
            .collect()
    }

    /// Get all NO levels (asks) as a vector sorted by price ascending
    pub fn get_no_levels(&self) -> Vec<(u8, i64)> {
        self.no_levels
            .iter()
            .map(|(&price, &qty)| (price, qty))
            .collect()
    }

    /// Get the spread in cents
    pub fn spread(&self) -> u8 {
        let (bid, ask) = self.best_bid_ask();
        if ask > bid {
            ask - bid
        } else {
            0
        }
    }

    /// Check if the book is valid (no crossed market)
    fn validate_book(&self) -> Result<()> {
        let (best_bid, best_ask) = self.best_bid_ask();
        
        // In a valid Kalshi book, YES + NO prices should sum to 100 cents
        // And best_bid should be <= best_ask
        if best_bid > 0 && best_ask < 100 && best_bid >= best_ask {
            eprintln!("Warning: Potentially crossed market - bid: {}, ask: {}", best_bid, best_ask);
        }

        Ok(())
    }

    /// Get the timestamp of the last update
    pub fn last_update_ts(&self) -> u64 {
        self.last_update_ts
    }

    /// Check if the book is empty
    pub fn is_empty(&self) -> bool {
        self.yes_levels.is_empty() && self.no_levels.is_empty()
    }

    /// Get the total quantity on the YES side
    pub fn total_yes_quantity(&self) -> i64 {
        self.yes_levels.values().sum()
    }

    /// Get the total quantity on the NO side
    pub fn total_no_quantity(&self) -> i64 {
        self.no_levels.values().sum()
    }

    /// Calculate the mid price
    pub fn mid_price(&self) -> Option<f64> {
        let (bid, ask) = self.best_bid_ask();
        if bid > 0 && ask < 100 {
            Some((bid as f64 + ask as f64) / 2.0)
        } else {
            None
        }
    }

    /// Get the weighted average price on a side within a quantity
    pub fn get_weighted_avg_price(&self, side: Side, quantity: i64) -> Option<f64> {
        let levels = match side {
            Side::Yes => self.get_yes_levels(),
            Side::No => self.get_no_levels(),
        };

        let mut remaining_qty = quantity;
        let mut total_cost = 0.0;
        let mut total_qty = 0i64;

        for (price, qty) in levels {
            if remaining_qty <= 0 {
                break;
            }

            let qty_to_take = remaining_qty.min(qty);
            total_cost += price as f64 * qty_to_take as f64;
            total_qty += qty_to_take;
            remaining_qty -= qty_to_take;
        }

        if total_qty > 0 {
            Some(total_cost / total_qty as f64)
        } else {
            None
        }
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

/// Update an orderbook with an event
pub fn update_book_with_event(book: &mut OrderBook, event: &Event) -> Result<()> {
    match event {
        Event::Snapshot { yes_levels, no_levels, ts, .. } => {
            book.apply_snapshot(yes_levels.clone(), no_levels.clone(), *ts)?;
        }
        Event::Delta { side, price, delta, ts, .. } => {
            book.apply_delta(*side, *price, *delta, *ts)?;
        }
        Event::Trade { .. } => {
            // Trades don't update the book directly, but could be used for validation
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orderbook_snapshot() {
        let mut book = OrderBook::new();
        
        let yes_levels = vec![(48, 100), (47, 200)];
        let no_levels = vec![(52, 150), (53, 250)];
        
        book.apply_snapshot(yes_levels, no_levels, 1000).unwrap();
        
        let (bid, ask) = book.best_bid_ask();
        assert_eq!(bid, 48); // Highest YES price
        assert_eq!(ask, 52); // Lowest NO price
        assert_eq!(book.spread(), 4);
    }

    #[test]
    fn test_orderbook_delta() {
        let mut book = OrderBook::new();
        
        // Initialize with snapshot
        book.apply_snapshot(vec![(48, 100)], vec![(52, 150)], 1000).unwrap();
        
        // Add more quantity to YES side
        book.apply_delta(Side::Yes, 48, 50, 1001).unwrap();
        assert_eq!(book.get_quantity(Side::Yes, 48), 150);
        
        // Remove quantity
        book.apply_delta(Side::Yes, 48, -100, 1002).unwrap();
        assert_eq!(book.get_quantity(Side::Yes, 48), 50);
        
        // Remove entire level
        book.apply_delta(Side::Yes, 48, -50, 1003).unwrap();
        assert_eq!(book.get_quantity(Side::Yes, 48), 0);
        assert!(book.yes_levels.is_empty());
    }

    #[test]
    fn test_best_bid_ask() {
        let mut book = OrderBook::new();
        
        let yes_levels = vec![(45, 100), (46, 200), (47, 150)];
        let no_levels = vec![(53, 150), (54, 100), (52, 200)];
        
        book.apply_snapshot(yes_levels, no_levels, 1000).unwrap();
        
        let (bid, ask) = book.best_bid_ask();
        assert_eq!(bid, 47); // Highest YES
        assert_eq!(ask, 52); // Lowest NO
        
        assert_eq!(book.best_bid(), Some((47, 150)));
        assert_eq!(book.best_ask(), Some((52, 200)));
    }

    #[test]
    fn test_weighted_avg_price() {
        let mut book = OrderBook::new();
        
        let yes_levels = vec![(48, 100), (47, 200), (46, 300)];
        let no_levels = vec![(52, 150), (53, 250)];
        
        book.apply_snapshot(yes_levels, no_levels, 1000).unwrap();
        
        // Test YES side - should take from best price first
        let avg_price = book.get_weighted_avg_price(Side::Yes, 250).unwrap();
        let expected = (48.0 * 100.0 + 47.0 * 150.0) / 250.0;
        assert!((avg_price - expected).abs() < 0.001);
        
        // Test NO side
        let avg_price = book.get_weighted_avg_price(Side::No, 200).unwrap();
        let expected = (52.0 * 150.0 + 53.0 * 50.0) / 200.0;
        assert!((avg_price - expected).abs() < 0.001);
    }

    #[test]
    fn test_mid_price() {
        let mut book = OrderBook::new();
        
        book.apply_snapshot(vec![(48, 100)], vec![(52, 150)], 1000).unwrap();
        
        assert_eq!(book.mid_price(), Some(50.0));
    }
} 