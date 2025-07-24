use iced::{
    widget::{canvas, checkbox, column, container, row, scrollable, text, Canvas, slider},
    Application, Color, Command, Element, Length, Point, Rectangle, Settings, Theme,
};
use crate::kalshi_types::*;

#[derive(Debug, Clone)]
pub struct VisualizationData {
    pub orderbook_snapshots: Vec<OrderbookSnapshot>,
    pub strategy_actions: Vec<StrategyAction>,
    pub fills: Vec<Fill>,
}

#[derive(Debug, Clone)]
pub struct OrderbookSnapshot {
    pub ts: u64,
    pub bid: u8,
    pub ask: u8,
    pub spread: u8,
}

#[derive(Debug, Clone)]
pub enum StrategyAction {
    PlaceOrder { ts: u64, side: Side, price: u8, qty: i64 },
    CancelOrder { ts: u64, id: OrderId },
}

pub struct OrderbookVisualizer {
    data: VisualizationData,
    show_orderbook: bool,
    show_fills: bool,
    show_orders: bool,
    show_spread: bool,
    zoom_factor: f32,
    time_offset: f32,
    selected_timeframe: TimeFrame,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeFrame {
    Full,
    AroundFills,
    Custom,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleOrderbook,
    ToggleFills,
    ToggleOrders,
    ToggleSpread,
    ZoomChanged(f32),
    TimeOffsetChanged(f32),
    TimeFrameChanged(TimeFrame),
}

impl Application for OrderbookVisualizer {
    type Message = Message;
    type Theme = Theme;
    type Executor = iced::executor::Default;
    type Flags = VisualizationData;

    fn new(data: Self::Flags) -> (Self, Command<Self::Message>) {
        (
            Self {
                data,
                show_orderbook: true,
                show_fills: true,
                show_orders: true,
                show_spread: true,
                zoom_factor: 1.0,
                time_offset: 0.0,
                selected_timeframe: TimeFrame::Full,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        "Kalshi Orderbook Visualizer - InkBack".to_string()
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::ToggleOrderbook => self.show_orderbook = !self.show_orderbook,
            Message::ToggleFills => self.show_fills = !self.show_fills,
            Message::ToggleOrders => self.show_orders = !self.show_orders,
            Message::ToggleSpread => self.show_spread = !self.show_spread,
            Message::ZoomChanged(zoom) => self.zoom_factor = zoom,
            Message::TimeOffsetChanged(offset) => self.time_offset = offset,
            Message::TimeFrameChanged(timeframe) => self.selected_timeframe = timeframe,
        }
        Command::none()
    }

    fn view(&self) -> Element<Self::Message> {
        let chart = Canvas::new(OrderbookChartRenderer {
            data: &self.data,
            show_orderbook: self.show_orderbook,
            show_fills: self.show_fills,
            show_orders: self.show_orders,
            show_spread: self.show_spread,
            zoom_factor: self.zoom_factor,
            time_offset: self.time_offset,
        })
        .width(Length::FillPortion(4))
        .height(Length::Fill);

        let controls = self.create_controls();

        row![
            chart,
            container(controls)
                .width(Length::FillPortion(1))
                .padding(20)
        ]
        .into()
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }
}

impl OrderbookVisualizer {
    fn create_controls(&self) -> Element<Message> {
        let mut controls = column![
            text("Kalshi Orderbook Visualizer").size(24),
            text("Toggle Display Elements:").size(16),
        ]
        .spacing(15);

        // Toggle checkboxes
        controls = controls.push(
            checkbox("Show Bid/Ask Lines", self.show_orderbook)
                .on_toggle(|_| Message::ToggleOrderbook)
        );

        controls = controls.push(
            checkbox("Show Fills", self.show_fills)
                .on_toggle(|_| Message::ToggleFills)
        );

        controls = controls.push(
            checkbox("Show Strategy Orders", self.show_orders)
                .on_toggle(|_| Message::ToggleOrders)
        );

        controls = controls.push(
            checkbox("Show Spread", self.show_spread)
                .on_toggle(|_| Message::ToggleSpread)
        );

        // Zoom control
        controls = controls.push(text("Zoom:").size(14));
        controls = controls.push(
            slider(0.1..=5.0, self.zoom_factor, Message::ZoomChanged)
        );

        // Time offset control
        controls = controls.push(text("Time Offset:").size(14));
        controls = controls.push(
            slider(-1.0..=1.0, self.time_offset, Message::TimeOffsetChanged)
        );

        // Statistics
        controls = controls.push(text("Statistics:").size(16));
        controls = controls.push(text(format!("Snapshots: {}", self.data.orderbook_snapshots.len())).size(12));
        controls = controls.push(text(format!("Strategy Actions: {}", self.data.strategy_actions.len())).size(12));
        controls = controls.push(text(format!("Fills: {}", self.data.fills.len())).size(12));

        if !self.data.fills.is_empty() {
            let total_filled_qty: i64 = self.data.fills.iter().map(|f| f.qty).sum();
            let avg_fill_price: f64 = self.data.fills.iter().map(|f| f.price as f64).sum::<f64>() / self.data.fills.len() as f64;
            controls = controls.push(text(format!("Total Volume: {}", total_filled_qty)).size(12));
            controls = controls.push(text(format!("Avg Fill Price: {:.1}¢", avg_fill_price)).size(12));
        }

        if !self.data.orderbook_snapshots.is_empty() {
            let avg_spread: f64 = self.data.orderbook_snapshots.iter().map(|s| s.spread as f64).sum::<f64>() / self.data.orderbook_snapshots.len() as f64;
            controls = controls.push(text(format!("Avg Spread: {:.1}¢", avg_spread)).size(12));
        }

        scrollable(controls).into()
    }
}

struct OrderbookChartRenderer<'a> {
    data: &'a VisualizationData,
    show_orderbook: bool,
    show_fills: bool,
    show_orders: bool,
    show_spread: bool,
    zoom_factor: f32,
    time_offset: f32,
}

impl<'a> canvas::Program<Message> for OrderbookChartRenderer<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        if self.data.orderbook_snapshots.is_empty() {
            // Draw "No Data" message
            let no_data_text = canvas::Text {
                content: "No orderbook data available".to_string(),
                position: Point::new(bounds.width / 2.0, bounds.height / 2.0),
                color: Color::WHITE,
                size: iced::Pixels(16.0),
                horizontal_alignment: iced::alignment::Horizontal::Center,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            };
            frame.fill_text(no_data_text);
            return vec![frame.into_geometry()];
        }

        // Chart margins
        let margin = 60.0;
        let chart_bounds = Rectangle {
            x: margin,
            y: margin,
            width: bounds.width - 2.0 * margin,
            height: bounds.height - 2.0 * margin,
        };

        // Calculate time and price ranges
        let (time_range, price_range) = self.calculate_ranges();

        // Draw grid and axes
        self.draw_grid_and_axes(&mut frame, &chart_bounds, &time_range, &price_range);

        // Draw orderbook data
        if self.show_orderbook {
            self.draw_orderbook_lines(&mut frame, &chart_bounds, &time_range, &price_range);
        }

        if self.show_spread {
            self.draw_spread_area(&mut frame, &chart_bounds, &time_range, &price_range);
        }

        // Draw fills
        if self.show_fills {
            self.draw_fills(&mut frame, &chart_bounds, &time_range, &price_range);
        }

        // Draw strategy orders
        if self.show_orders {
            self.draw_strategy_orders(&mut frame, &chart_bounds, &time_range, &price_range);
        }

        vec![frame.into_geometry()]
    }
}

impl<'a> OrderbookChartRenderer<'a> {
    fn calculate_ranges(&self) -> ((u64, u64), (u8, u8)) {
        let snapshots = &self.data.orderbook_snapshots;
        
        if snapshots.is_empty() {
            return ((0, 1), (0, 100));
        }

        let min_time = snapshots.first().unwrap().ts;
        let max_time = snapshots.last().unwrap().ts;
        
        let min_price = snapshots.iter().map(|s| s.bid.min(s.ask)).min().unwrap_or(0);
        let max_price = snapshots.iter().map(|s| s.bid.max(s.ask)).max().unwrap_or(100);
        
        // Add some padding to price range
        let price_padding = ((max_price - min_price) as f32 * 0.1) as u8;
        let padded_min_price = min_price.saturating_sub(price_padding);
        let padded_max_price = (max_price + price_padding).min(100);

        ((min_time, max_time), (padded_min_price, padded_max_price))
    }

    fn draw_grid_and_axes(
        &self,
        frame: &mut canvas::Frame,
        bounds: &Rectangle,
        time_range: &(u64, u64),
        price_range: &(u8, u8),
    ) {
        use iced::widget::canvas::{Path, Stroke, Text};

        let stroke = Stroke::default().with_width(1.0).with_color(Color::from_rgb(0.3, 0.3, 0.3));

        // Draw axes
        let y_axis = Path::line(
            Point::new(bounds.x, bounds.y),
            Point::new(bounds.x, bounds.y + bounds.height),
        );
        frame.stroke(&y_axis, stroke.clone());

        let x_axis = Path::line(
            Point::new(bounds.x, bounds.y + bounds.height),
            Point::new(bounds.x + bounds.width, bounds.y + bounds.height),
        );
        frame.stroke(&x_axis, stroke);

        // Draw grid lines
        let grid_stroke = Stroke::default().with_width(0.5).with_color(Color::from_rgb(0.2, 0.2, 0.2));

        // Horizontal grid lines (price levels)
        for i in 0..=10 {
            let y_ratio = i as f32 / 10.0;
            let y = bounds.y + bounds.height * (1.0 - y_ratio);
            let price = price_range.0 as f32 + (price_range.1 - price_range.0) as f32 * y_ratio;

            let grid_line = Path::line(
                Point::new(bounds.x, y),
                Point::new(bounds.x + bounds.width, y),
            );
            frame.stroke(&grid_line, grid_stroke.clone());

            // Price labels
            let label = Text {
                content: format!("{:.0}¢", price),
                position: Point::new(bounds.x - 5.0, y),
                color: Color::WHITE,
                size: iced::Pixels(10.0),
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            };
            frame.fill_text(label);
        }

        // Vertical grid lines (time)
        for i in 0..=10 {
            let x_ratio = i as f32 / 10.0;
            let x = bounds.x + bounds.width * x_ratio;
            let time = time_range.0 as f32 + (time_range.1 - time_range.0) as f32 * x_ratio;

            let grid_line = Path::line(
                Point::new(x, bounds.y),
                Point::new(x, bounds.y + bounds.height),
            );
            frame.stroke(&grid_line, grid_stroke.clone());

            // Time labels
            let label = Text {
                content: format!("{:.0}", time),
                position: Point::new(x, bounds.y + bounds.height + 15.0),
                color: Color::WHITE,
                size: iced::Pixels(10.0),
                horizontal_alignment: iced::alignment::Horizontal::Center,
                vertical_alignment: iced::alignment::Vertical::Top,
                ..Default::default()
            };
            frame.fill_text(label);
        }

        // Axis labels
        let price_label = Text {
            content: "Price (cents)".to_string(),
            position: Point::new(20.0, bounds.height / 2.0),
            color: Color::WHITE,
            size: iced::Pixels(12.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        };
        frame.fill_text(price_label);

        let time_label = Text {
            content: "Time".to_string(),
            position: Point::new(bounds.x + bounds.width / 2.0, bounds.y + bounds.height + 40.0),
            color: Color::WHITE,
            size: iced::Pixels(12.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        };
        frame.fill_text(time_label);
    }

    fn draw_orderbook_lines(
        &self,
        frame: &mut canvas::Frame,
        bounds: &Rectangle,
        time_range: &(u64, u64),
        price_range: &(u8, u8),
    ) {
        use iced::widget::canvas::{Path, Stroke};

        if self.data.orderbook_snapshots.len() < 2 {
            return;
        }

        let time_span = (time_range.1 - time_range.0) as f32;
        let price_span = (price_range.1 - price_range.0) as f32;

        // Draw bid line (green)
        let bid_path = Path::new(|builder| {
            for (i, snapshot) in self.data.orderbook_snapshots.iter().enumerate() {
                let x = bounds.x + ((snapshot.ts - time_range.0) as f32 / time_span) * bounds.width;
                let y = bounds.y + bounds.height * (1.0 - (snapshot.bid - price_range.0) as f32 / price_span);

                if i == 0 {
                    builder.move_to(Point::new(x, y));
                } else {
                    builder.line_to(Point::new(x, y));
                }
            }
        });

        let bid_stroke = Stroke::default().with_width(2.0).with_color(Color::from_rgb(0.0, 1.0, 0.0));
        frame.stroke(&bid_path, bid_stroke);

        // Draw ask line (red)
        let ask_path = Path::new(|builder| {
            for (i, snapshot) in self.data.orderbook_snapshots.iter().enumerate() {
                let x = bounds.x + ((snapshot.ts - time_range.0) as f32 / time_span) * bounds.width;
                let y = bounds.y + bounds.height * (1.0 - (snapshot.ask - price_range.0) as f32 / price_span);

                if i == 0 {
                    builder.move_to(Point::new(x, y));
                } else {
                    builder.line_to(Point::new(x, y));
                }
            }
        });

        let ask_stroke = Stroke::default().with_width(2.0).with_color(Color::from_rgb(1.0, 0.0, 0.0));
        frame.stroke(&ask_path, ask_stroke);
    }

    fn draw_spread_area(
        &self,
        frame: &mut canvas::Frame,
        bounds: &Rectangle,
        time_range: &(u64, u64),
        price_range: &(u8, u8),
    ) {
        use iced::widget::canvas::{Path, Fill};

        if self.data.orderbook_snapshots.len() < 2 {
            return;
        }

        let time_span = (time_range.1 - time_range.0) as f32;
        let price_span = (price_range.1 - price_range.0) as f32;

        // Draw spread area (light blue)
        let spread_path = Path::new(|builder| {
            // Draw top edge (ask line)
            for (i, snapshot) in self.data.orderbook_snapshots.iter().enumerate() {
                let x = bounds.x + ((snapshot.ts - time_range.0) as f32 / time_span) * bounds.width;
                let y = bounds.y + bounds.height * (1.0 - (snapshot.ask - price_range.0) as f32 / price_span);

                if i == 0 {
                    builder.move_to(Point::new(x, y));
                } else {
                    builder.line_to(Point::new(x, y));
                }
            }

            // Draw bottom edge (bid line) in reverse
            for snapshot in self.data.orderbook_snapshots.iter().rev() {
                let x = bounds.x + ((snapshot.ts - time_range.0) as f32 / time_span) * bounds.width;
                let y = bounds.y + bounds.height * (1.0 - (snapshot.bid - price_range.0) as f32 / price_span);
                builder.line_to(Point::new(x, y));
            }

            builder.close();
        });

        let spread_fill = Fill::from(Color::from_rgba(0.0, 0.5, 1.0, 0.2));
        frame.fill(&spread_path, spread_fill);
    }

    fn draw_fills(
        &self,
        frame: &mut canvas::Frame,
        bounds: &Rectangle,
        time_range: &(u64, u64),
        price_range: &(u8, u8),
    ) {
        use iced::widget::canvas::{Path, Fill, Stroke};

        let time_span = (time_range.1 - time_range.0) as f32;
        let price_span = (price_range.1 - price_range.0) as f32;

        for fill in &self.data.fills {
            let x = bounds.x + ((fill.ts - time_range.0) as f32 / time_span) * bounds.width;
            let y = bounds.y + bounds.height * (1.0 - (fill.price - price_range.0) as f32 / price_span);

            // Draw fill as a circle
            let circle = Path::circle(Point::new(x, y), 6.0);
            let fill_color = Fill::from(Color::from_rgb(1.0, 1.0, 0.0)); // Yellow
            let stroke_color = Stroke::default().with_width(2.0).with_color(Color::from_rgb(0.8, 0.8, 0.0));
            
            frame.fill(&circle, fill_color);
            frame.stroke(&circle, stroke_color);
        }
    }

    fn draw_strategy_orders(
        &self,
        frame: &mut canvas::Frame,
        bounds: &Rectangle,
        time_range: &(u64, u64),
        price_range: &(u8, u8),
    ) {
        use iced::widget::canvas::{Path, Stroke};

        let time_span = (time_range.1 - time_range.0) as f32;
        let price_span = (price_range.1 - price_range.0) as f32;

        for action in &self.data.strategy_actions {
            match action {
                StrategyAction::PlaceOrder { ts, side, price, .. } => {
                    let x = bounds.x + ((*ts - time_range.0) as f32 / time_span) * bounds.width;
                    let y = bounds.y + bounds.height * (1.0 - (*price - price_range.0) as f32 / price_span);

                    // Draw order placement as a triangle
                    let triangle = Path::new(|builder| {
                        let size = 8.0;
                        match side {
                            Side::Yes => {
                                // Up triangle for buy orders (YES)
                                builder.move_to(Point::new(x, y - size));
                                builder.line_to(Point::new(x - size, y + size));
                                builder.line_to(Point::new(x + size, y + size));
                                builder.close();
                            }
                            Side::No => {
                                // Down triangle for sell orders (NO)
                                builder.move_to(Point::new(x, y + size));
                                builder.line_to(Point::new(x - size, y - size));
                                builder.line_to(Point::new(x + size, y - size));
                                builder.close();
                            }
                        }
                    });

                    let color = match side {
                        Side::Yes => Color::from_rgb(0.0, 1.0, 0.5), // Green-ish
                        Side::No => Color::from_rgb(1.0, 0.5, 0.0),  // Orange-ish
                    };

                    let stroke = Stroke::default().with_width(2.0).with_color(color);
                    frame.stroke(&triangle, stroke);
                }
                StrategyAction::CancelOrder { ts, .. } => {
                    let x = bounds.x + ((*ts - time_range.0) as f32 / time_span) * bounds.width;

                    // Draw cancel as a vertical line
                    let cancel_line = Path::line(
                        Point::new(x, bounds.y),
                        Point::new(x, bounds.y + bounds.height),
                    );

                    let stroke = Stroke::default().with_width(1.0).with_color(Color::from_rgb(1.0, 0.0, 1.0)); // Magenta
                    frame.stroke(&cancel_line, stroke);
                }
            }
        }
    }
}

pub fn run_visualizer(data: VisualizationData) -> iced::Result {
    OrderbookVisualizer::run(Settings::with_flags(data))
} 