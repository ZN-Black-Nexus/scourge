//! Screen state machine and drawing. ratatui diffs frames internally, so
//! only cells that actually changed hit the terminal - no full-screen clear
//! flicker on every move.

use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use scourge_core::{Direction, Tile, World};

use crate::keybindings::{describe_key, KeyBindings, DIRECTIONS};

pub const PAUSE_ITEMS: [&str; 3] = ["Resume", "Settings", "Quit"];
pub const SETTINGS_ITEMS: [&str; 3] = [
    "Movement: Vi-keys (hjkl + yubn)",
    "Movement: WASD (wasd + qezc)",
    "Custom keybinds...",
];

pub enum Screen {
    Game,
    Pause { selected: usize },
    Settings { selected: usize },
    CustomBinds { selected: usize },
    AwaitingKey { direction: Direction },
}

pub fn draw(frame: &mut Frame, world: &World, screen: &Screen, binds: &KeyBindings) {
    draw_game(frame, world);
    match screen {
        Screen::Game => {}
        Screen::Pause { selected } => draw_menu(frame, "Paused", &PAUSE_ITEMS, *selected),
        Screen::Settings { selected } => draw_menu(frame, "Settings", &SETTINGS_ITEMS, *selected),
        Screen::CustomBinds { selected } => draw_custom_binds(frame, binds, *selected),
        Screen::AwaitingKey { direction } => draw_awaiting_key(frame, *direction),
    }
}

fn draw_game(frame: &mut Frame, world: &World) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);

    let lines: Vec<Line> = (0..world.map.height)
        .map(|y| {
            let row: String = (0..world.map.width)
                .map(|x| {
                    if x == world.player.pos.x && y == world.player.pos.y {
                        '@'
                    } else {
                        world.map.tile(x, y).map(Tile::glyph).unwrap_or(' ')
                    }
                })
                .collect();
            Line::raw(row)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(0, 0, area.width, map_height),
    );

    let status = format!("turn {} - Esc for menu", world.turn);
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(0, map_height, area.width, 1),
    );
}

fn draw_menu(frame: &mut Frame, title: &str, items: &[&str], selected: usize) {
    let popup = centered_rect(50, 40, frame.area());
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if i == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(*label).style(style)
        })
        .collect();
    let list = List::new(list_items).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

fn draw_custom_binds(frame: &mut Frame, binds: &KeyBindings, selected: usize) {
    let popup = centered_rect(55, 60, frame.area());
    let items: Vec<ListItem> = DIRECTIONS
        .iter()
        .enumerate()
        .map(|(i, dir)| {
            let key = binds
                .key_for(*dir)
                .map(describe_key)
                .unwrap_or_else(|| "unbound".to_string());
            let style = if i == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(format!("{dir:?}: {key}")).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .title("Custom Keybinds (Enter to rebind, Esc to go back)")
            .borders(Borders::ALL),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

fn draw_awaiting_key(frame: &mut Frame, direction: Direction) {
    let popup = centered_rect(50, 20, frame.area());
    let text = format!("Press a key to bind to {direction:?} (Esc to cancel)");
    let paragraph =
        Paragraph::new(text).block(Block::default().title("Rebind").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
