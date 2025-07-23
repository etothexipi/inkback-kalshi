use crate::kalshi_types::{Event, Side};
use anyhow::Result;
use std::collections::BTreeMap;

/// L2 order book for Kalshi YES/NO contracts
/// YES levels are bids, NO levels are asks (with price inverted to 100-price)
#[derive(Debug, Clone)]
pub struct OrderBook {
    /// YES side levels (bids) - price -> quantity, sorted ascending
    yes_levels: BTreeMap<u8, i64>,
    /// NO side levels (asks) - price -> quantity, sorted ascending
    /// Note: NO prices are stored as-is, but converted to (100-price) when getting asks
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

        // Add YES levels (bids)
        for (price, qty) in yes_levels {
            if qty > 0 {
                self.yes_levels.insert(price, qty);
            }
        }

        // Add NO levels (asks) - store as-is, will convert when needed
        for (price, qty) in no_levels {
            if qty > 0 {
                self.no_levels.insert(price, qty);
            }
        }

        self.last_update_ts = ts;
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
        Ok(())
    }

    /// Get the best bid and ask prices in cents
    /// Returns (bid_price, ask_price) where:
    /// - bid_price is the highest YES price (best bid)
    /// - ask_price is the lowest inverted NO price (best ask)
    pub fn best_bid_ask(&self) -> (u8, u8) {
        let best_bid = self.yes_levels
            .iter()
            .rev() // Highest price first
            .next()
            .map(|(&price, _)| price)
            .unwrap_or(0);

        // For NO side, we need to find the lowest (100-price) value
        // This means finding the highest NO price and inverting it
        let best_ask = self.no_levels
            .iter()
            .rev() // Highest NO price first
            .next()
            .map(|(&price, _)| 100 - price) // Invert the price
            .unwrap_or(100);

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
            .rev() // Highest NO price = lowest ask price when inverted
            .next()
            .map(|(&price, &qty)| (100 - price, qty)) // Invert the price
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

    /// Get all NO levels (asks) as a vector sorted by price ascending (after inversion)
    pub fn get_no_levels(&self) -> Vec<(u8, i64)> {
        let mut inverted_levels: Vec<(u8, i64)> = self.no_levels
            .iter()
            .map(|(&price, &qty)| (100 - price, qty)) // Invert NO prices
            .collect();
        
        // Sort by inverted price ascending (lowest ask first)
        inverted_levels.sort_by_key(|(price, _)| *price);
        inverted_levels
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
        
        // In a valid book, best_bid should be < best_ask
        if best_bid > 0 && best_ask < 100 && best_bid >= best_ask {
            println!("Warning: Potentially crossed market - bid: {}, ask: {}", best_bid, best_ask);
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

    /// Debug function to print the orderbook state
    pub fn debug_print(&self) {
        println!("=== Orderbook Debug ===");
        println!("YES levels (bids): {:?}", self.yes_levels);
        println!("NO levels (raw): {:?}", self.no_levels);
        println!("NO levels (inverted asks): {:?}", self.get_no_levels());
        let (bid, ask) = self.best_bid_ask();
        println!("Best bid: {}, Best ask: {}, Spread: {}", bid, ask, self.spread());
        println!("=======================");
    }

    /// Get the number of YES levels (bids)
    pub fn yes_level_count(&self) -> usize {
        self.yes_levels.len()
    }

    /// Get the number of NO levels (asks)
    pub fn no_level_count(&self) -> usize {
        self.no_levels.len()
    }

    /// Condensed debug view showing just the best levels
    pub fn debug_print_top(&self) {
        let (bid, ask) = self.best_bid_ask();
        
        // Get top 5 bid levels
        let top_bids: Vec<_> = self.yes_levels
            .iter()
            .rev()
            .take(5)
            .map(|(&price, &qty)| format!("{}@{}", qty, price))
            .collect();
            
        // Get top 5 ask levels (from inverted NO levels)
        let top_asks: Vec<_> = self.get_no_levels()
            .iter()
            .take(5)
            .map(|(price, qty)| format!("{}@{}", qty, price))
            .collect();

        println!("📊 {} | Bids: [{}] | Asks: [{}] | Spread: {}", 
                self.yes_levels.len() + self.no_levels.len(),
                top_bids.join(", "), 
                top_asks.join(", "), 
                self.spread());
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

/// Update an orderbook with an event (snapshot, delta, or trade)
pub fn update_book_with_event(book: &mut OrderBook, event: &Event) -> Result<()> {
    match event {
        Event::Snapshot { yes_levels, no_levels, ts, .. } => {
            book.apply_snapshot(yes_levels.clone(), no_levels.clone(), *ts)?;
        }
        Event::Delta { side, price, delta, ts, .. } => {
            book.apply_delta(*side, *price, *delta, *ts)?;
        }
        Event::Trade { .. } => {
            // Trades don't update the orderbook directly
            // They might be processed separately for fills
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