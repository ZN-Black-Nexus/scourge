//! Screen state machine and drawing. ratatui diffs frames internally, so
//! only cells that actually changed hit the terminal - no full-screen clear
//! flicker on every move.

use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use scourge_core::{Direction, Pos, Tile, World};
use scourge_render::{within_radius, Camera, Explored};

use crate::keybindings::{describe_key, KeyBindings, DIRECTIONS};

/// How far the player can currently see. Doesn't yet account for light
/// level, perception, or walls blocking line of sight - just distance.
const SIGHT_RADIUS: i32 = 10;

pub const PAUSE_ITEMS: [&str; 3] = ["Resume", "Settings", "Quit"];
pub const SETTINGS_ITEMS: [&str; 3] = [
    "Movement: Vi-keys (hjkl + yubn)",
    "Movement: WASD (wasd + qezc)",
    "Custom keybinds...",
];

pub enum Screen {
    Game,
    Map,
    Pause { selected: usize },
    Settings { selected: usize },
    CustomBinds { selected: usize },
    AwaitingKey { direction: Direction },
}

pub fn draw(
    frame: &mut Frame,
    world: &World,
    screen: &Screen,
    binds: &KeyBindings,
    camera: &mut Camera,
    explored: &mut Explored,
) {
    match screen {
        Screen::Map => draw_map_screen(frame, world, explored),
        _ => draw_game(frame, world, camera, explored),
    }
    match screen {
        Screen::Game | Screen::Map => {}
        Screen::Pause { selected } => draw_menu(frame, "Paused", &PAUSE_ITEMS, *selected),
        Screen::Settings { selected } => draw_menu(frame, "Settings", &SETTINGS_ITEMS, *selected),
        Screen::CustomBinds { selected } => draw_custom_binds(frame, binds, *selected),
        Screen::AwaitingKey { direction } => draw_awaiting_key(frame, *direction),
    }
}

/// One visible cell: its glyph, and whether it's in current view (full
/// brightness) or only remembered (dimmed).
fn cell(world: &World, explored: &Explored, x: i32, y: i32) -> (char, bool) {
    if x == world.player.pos.x && y == world.player.pos.y {
        return ('@', true);
    }
    let visible = within_radius(Pos { x, y }, world.player.pos, SIGHT_RADIUS);
    if visible {
        (world.map.tile(x, y).map(Tile::glyph).unwrap_or(' '), true)
    } else if explored.is_seen(x, y) {
        (world.map.tile(x, y).map(Tile::glyph).unwrap_or(' '), false)
    } else {
        (' ', false)
    }
}

/// Builds one row as styled spans, grouping consecutive same-style cells so
/// a normal move only touches a handful of runs, not one span per character.
fn build_row(cells: impl Iterator<Item = (char, bool)>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_visible = true;
    for (ch, visible) in cells {
        if current.is_empty() {
            current_visible = visible;
        } else if visible != current_visible {
            spans.push(span_for(std::mem::take(&mut current), current_visible));
            current_visible = visible;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        spans.push(span_for(current, current_visible));
    }
    Line::from(spans)
}

fn span_for(text: String, visible: bool) -> Span<'static> {
    if visible {
        Span::raw(text)
    } else {
        Span::styled(text, Style::default().add_modifier(Modifier::DIM))
    }
}

fn draw_game(frame: &mut Frame, world: &World, camera: &mut Camera, explored: &mut Explored) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);
    let (viewport_w, viewport_h) = (area.width as i32, map_height as i32);

    camera.center_on(world.player.pos, viewport_w, viewport_h, world.map.width, world.map.height);
    explored.reveal(world.player.pos, SIGHT_RADIUS);

    let lines: Vec<Line> = (0..viewport_h)
        .map(|screen_y| {
            let world_y = camera.y + screen_y;
            build_row((0..viewport_w).map(|screen_x| cell(world, explored, camera.x + screen_x, world_y)))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(0, 0, area.width, map_height),
    );

    let status = format!("turn {} - Esc for menu, m for map", world.turn);
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(0, map_height, area.width, 1),
    );
}

/// The 'm' overview: everything explored so far around the player, ignoring
/// current sight radius. Not yet scrollable/zoomed beyond the normal
/// viewport - future work if the world grows large enough to need that.
fn draw_map_screen(frame: &mut Frame, world: &World, explored: &Explored) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);
    let (viewport_w, viewport_h) = (area.width as i32, map_height as i32);

    let cam_x = (world.player.pos.x - viewport_w / 2).clamp(0, (world.map.width - viewport_w).max(0));
    let cam_y = (world.player.pos.y - viewport_h / 2).clamp(0, (world.map.height - viewport_h).max(0));

    let lines: Vec<Line> = (0..viewport_h)
        .map(|screen_y| {
            let world_y = cam_y + screen_y;
            let row: String = (0..viewport_w)
                .map(|screen_x| {
                    let world_x = cam_x + screen_x;
                    if world_x == world.player.pos.x && world_y == world.player.pos.y {
                        '@'
                    } else if explored.is_seen(world_x, world_y) {
                        world.map.tile(world_x, world_y).map(Tile::glyph).unwrap_or(' ')
                    } else {
                        ' '
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

    frame.render_widget(
        Paragraph::new("Map - Esc to return"),
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
