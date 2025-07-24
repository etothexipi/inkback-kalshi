use crate::kalshi_types::{Event, RawObRow, RawTradeRow};
use anyhow::Result;
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;

/// Merge orderbook and trades iterators into a unified event stream
pub fn merge<OB, TR>(
    orderbook_iter: OB,
    trades_iter: TR,
) -> impl Iterator<Item = Result<Event>>
where
    OB: Iterator<Item = Result<RawObRow>>,
    TR: Iterator<Item = Result<RawTradeRow>>,
{
    EventMerger::new(orderbook_iter, trades_iter)
}

/// Internal struct for merging and ordering events
struct EventMerger<OB, TR>
where
    OB: Iterator<Item = Result<RawObRow>>,
    TR: Iterator<Item = Result<RawTradeRow>>,
{
    orderbook_iter: OB,
    trades_iter: TR,
    event_heap: BinaryHeap<Reverse<OrderedEvent>>,
    orderbook_exhausted: bool,
    trades_exhausted: bool,
    snapshot_buffer: HashMap<(u64, String), Vec<RawObRow>>, // Group by (seq, ticker)
}

/// Wrapper for events that implements ordering by (seq, ts)
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedEvent {
    seq: u64,
    ts: u64,
    event: Event,
}

impl PartialOrd for OrderedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // First by seq, then by ts
        self.seq.cmp(&other.seq).then_with(|| self.ts.cmp(&other.ts))
    }
}

impl<OB, TR> EventMerger<OB, TR>
where
    OB: Iterator<Item = Result<RawObRow>>,
    TR: Iterator<Item = Result<RawTradeRow>>,
{
    fn new(orderbook_iter: OB, trades_iter: TR) -> Self {
        Self {
            orderbook_iter,
            trades_iter,
            event_heap: BinaryHeap::new(),
            orderbook_exhausted: false,
            trades_exhausted: false,
            snapshot_buffer: HashMap::new(),
        }
    }

    fn fill_heap(&mut self) -> Result<()> {
        // Try to get more orderbook events
        if !self.orderbook_exhausted {
            match self.orderbook_iter.next() {
                Some(Ok(raw_row)) => {
                    self.process_orderbook_row(raw_row)?;
                }
                Some(Err(e)) => return Err(e),
                None => self.orderbook_exhausted = true,
            }
        }

        // Try to get more trade events
        if !self.trades_exhausted {
            match self.trades_iter.next() {
                Some(Ok(raw_trade)) => {
                    let event = Event::Trade {
                        seq: raw_trade.seq,
                        ts: raw_trade.ts,
                        ticker: raw_trade.ticker,
                        yes_price: raw_trade.yes_price,
                        no_price: raw_trade.no_price,
                        qty: raw_trade.qty,
                        taker: raw_trade.taker,
                    };
                    
                    self.event_heap.push(Reverse(OrderedEvent {
                        seq: raw_trade.seq,
                        ts: raw_trade.ts,
                        event,
                    }));
                }
                Some(Err(e)) => return Err(e),
                None => self.trades_exhausted = true,
            }
        }

        Ok(())
    }

    fn process_orderbook_row(&mut self, raw_row: RawObRow) -> Result<()> {
        match raw_row.msg_type.as_str() {
            "snapshot" => {
                // For snapshot rows, just add to buffer - don't flush other snapshots
                // This allows all snapshot rows with the same (seq, ticker) to accumulate
                let current_key = (raw_row.seq, raw_row.ticker.clone());
                
                self.snapshot_buffer.entry(current_key).or_insert_with(Vec::new).push(raw_row);
            }
            "delta" => {
                // Before processing ANY delta, flush ALL pending snapshots
                // This ensures snapshots are processed before deltas, regardless of ticker
                if !self.snapshot_buffer.is_empty() {
                    self.flush_all_snapshots()?;
                }
                
                let event = Event::Delta {
                    seq: raw_row.seq,
                    ts: raw_row.ts,
                    ticker: raw_row.ticker,
                    side: raw_row.side.expect("Delta must have side"),
                    price: raw_row.price.expect("Delta must have price"),
                    delta: raw_row.quantity.expect("Delta must have quantity"),
                };
                
                self.event_heap.push(Reverse(OrderedEvent {
                    seq: raw_row.seq,
                    ts: raw_row.ts,
                    event,
                }));
            }
            _ => return Err(anyhow::anyhow!("Unknown message type: {}", raw_row.msg_type)),
        }
        
        Ok(())
    }

    fn flush_all_snapshots(&mut self) -> Result<()> {
        let keys_to_flush: Vec<_> = self.snapshot_buffer.keys().cloned().collect();
        
        for key in keys_to_flush {
            if let Some(snapshot_rows) = self.snapshot_buffer.remove(&key) {
                self.create_snapshot_event(snapshot_rows)?;
            }
        }
        
        Ok(())
    }

    fn flush_complete_snapshots(&mut self) -> Result<()> {
        // This method is now only called at the end of the stream
        self.flush_all_snapshots()
    }

    fn create_snapshot_event(&mut self, snapshot_rows: Vec<RawObRow>) -> Result<()> {
        if snapshot_rows.is_empty() {
            return Ok(());
        }

        // Take the first row for basic info
        let first_row = &snapshot_rows[0];

        let mut yes_levels = Vec::new();
        let mut no_levels = Vec::new();

        for row in &snapshot_rows {
            if let (Some(side), Some(price), Some(quantity)) = (&row.side, row.price, row.quantity) {
                if quantity > 0 {  // Only include positive quantities
                    match side {
                        crate::kalshi_types::Side::Yes => {
                            yes_levels.push((price, quantity));
                        }
                        crate::kalshi_types::Side::No => {
                            no_levels.push((price, quantity));
                        }
                    }
                }
            }
        }

        // Sort levels by price
        yes_levels.sort_by_key(|&(price, _)| price);
        no_levels.sort_by_key(|&(price, _)| price);

        let event = Event::Snapshot {
            seq: first_row.seq,
            ts: first_row.ts,
            ticker: first_row.ticker.clone(),
            yes_levels,
            no_levels,
        };

        self.event_heap.push(Reverse(OrderedEvent {
            seq: first_row.seq,
            ts: first_row.ts,
            event,
        }));

        Ok(())
    }
}

impl<OB, TR> Iterator for EventMerger<OB, TR>
where
    OB: Iterator<Item = Result<RawObRow>>,
    TR: Iterator<Item = Result<RawTradeRow>>,
{
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Try to fill the heap with more events
            if let Err(e) = self.fill_heap() {
                return Some(Err(e));
            }

            // Return the next event from the heap
            if let Some(Reverse(ordered_event)) = self.event_heap.pop() {
                return Some(Ok(ordered_event.event));
            }

            // If heap is empty and both iterators are exhausted, we're done
            if self.orderbook_exhausted && self.trades_exhausted {
                // Flush any remaining snapshots
                if !self.snapshot_buffer.is_empty() {
                    let keys: Vec<_> = self.snapshot_buffer.keys().cloned().collect();
                    for key in keys {
                        if let Some(snapshot_rows) = self.snapshot_buffer.remove(&key) {
                            if let Err(e) = self.create_snapshot_event(snapshot_rows) {
                                return Some(Err(e));
                            }
                            // Return the snapshot if we created one
                            if let Some(Reverse(ordered_event)) = self.event_heap.pop() {
                                return Some(Ok(ordered_event.event));
                            }
                        }
                    }
                }
                return None;
            }
        }
    }
}

/// Validate that events are properly ordered by sequence number
pub fn validate_sequence_ordering<I>(events: I) -> Result<()>
where
    I: Iterator<Item = Result<Event>>,
{
    let mut last_seq: Option<u64> = None;
    
    for (index, event_result) in events.enumerate() {
        let event = event_result?;
        let seq = event.seq();
        
        if let Some(prev_seq) = last_seq {
            if seq < prev_seq {
                return Err(anyhow::anyhow!(
                    "Sequence regression at event {}: {} < {}", 
                    index, seq, prev_seq
                ));
            }
            if seq == prev_seq {
                // Same sequence number is allowed, but warn
                eprintln!("Warning: Duplicate sequence number {} at event {}", seq, index);
            }
        }
        
        last_seq = Some(seq);
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kalshi_types::{RawObRow, RawTradeRow};

    #[test]
    fn test_event_ordering() {
        let orderbook_rows = vec![
            Ok(RawObRow {
                seq: 1,
                ts: 1000,
                ticker: "TEST".to_string(),
                msg_type: "snapshot".to_string(),
                side: None,
                price: None,
                quantity: None,
                yes_levels: Some(vec![(50, 100)]),
                no_levels: Some(vec![(51, 200)]),
            }),
            Ok(RawObRow {
                seq: 3,
                ts: 3000,
                ticker: "TEST".to_string(),
                msg_type: "delta".to_string(),
                side: Some(Side::Yes),
                price: Some(50),
                quantity: Some(10),
                yes_levels: None,
                no_levels: None,
            }),
        ];

        let trade_rows = vec![
            Ok(RawTradeRow {
                seq: 2,
                ts: 2000,
                ticker: "TEST".to_string(),
                yes_price: 50,
                no_price: 50,
                qty: 25,
                taker: Side::Yes,
            }),
        ];

        let events: Vec<_> = merge(orderbook_rows.into_iter(), trade_rows.into_iter())
            .collect::<Result<Vec<_>>>()
            .unwrap();

        assert_eq!(events.len(), 3);
        
        // Events should be ordered by sequence number
        assert_eq!(events[0].seq(), 1); // Snapshot
        assert_eq!(events[1].seq(), 2); // Trade
        assert_eq!(events[2].seq(), 3); // Delta

        // Verify event types
        matches!(events[0], Event::Snapshot { .. });
        matches!(events[1], Event::Trade { .. });
        matches!(events[2], Event::Delta { .. });
    }

    #[test]
    fn test_sequence_validation() {
        let events = vec![
            Ok(Event::Snapshot { seq: 1, ts: 1000, ticker: "TEST".to_string(), yes_levels: vec![], no_levels: vec![] }),
            Ok(Event::Trade { seq: 2, ts: 2000, ticker: "TEST".to_string(), yes_price: 50, no_price: 50, qty: 25, taker: Side::Yes }),
            Ok(Event::Delta { seq: 3, ts: 3000, ticker: "TEST".to_string(), side: Side::Yes, price: 50, delta: 10 }),
        ];

        assert!(validate_sequence_ordering(events.into_iter()).is_ok());
    }

    #[test]
    fn test_sequence_validation_regression() {
        let events = vec![
            Ok(Event::Snapshot { seq: 1, ts: 1000, ticker: "TEST".to_string(), yes_levels: vec![], no_levels: vec![] }),
            Ok(Event::Trade { seq: 3, ts: 3000, ticker: "TEST".to_string(), yes_price: 50, no_price: 50, qty: 25, taker: Side::Yes }),
            Ok(Event::Delta { seq: 2, ts: 2000, ticker: "TEST".to_string(), side: Side::Yes, price: 50, delta: 10 }),
        ];

        assert!(validate_sequence_ordering(events.into_iter()).is_err());
    }
} 