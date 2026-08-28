//! Screen state machine and drawing. ratatui diffs frames internally, so
//! only cells that actually changed hit the terminal - no full-screen clear
//! flicker on every move.

use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use scourge_core::{Direction, Tile, VisibleSet, World};
use scourge_render::{Camera, Explored};

use crate::keybindings::{describe_key, KeyBindings, DIRECTIONS};

/// How far the player can see, before line-of-sight is even considered.
/// This is meant to rarely be the binding constraint - DDA characters can
/// typically see well past a normal terminal's width in daylight, so what
/// actually limits view is walls (see `Map::visible_from`), not this
/// number. Doesn't yet vary with light level or a perception stat.
pub(crate) const SIGHT_RADIUS: i32 = 60;

/// Tiles farther than this (but still within line of sight) render dimmed
/// rather than full-brightness - a rough stand-in for DDA's actual graduated
/// light-level falloff, which we don't model yet (no light sources, time of
/// day, or perception stat). Without that, "visible" was a hard on/off edge,
/// which reads as flatter/less natural than DDA's real vision.
const NEAR_RADIUS: i32 = 20;

pub const PAUSE_ITEMS: [&str; 3] = ["Resume", "Settings", "Quit"];
pub const SETTINGS_ITEMS: [&str; 3] = [
    "Movement: Vi-keys (hjkl + yubn)",
    "Movement: WASD (wasd + qezc)",
    "Custom keybinds...",
];

pub enum Screen {
    Game,
    /// Free-look cursor: uses your character's real current sight (not
    /// memory-only like the map screen), lets you inspect anything within
    /// it even off the normal viewport, without moving or costing a turn.
    Look { cursor_x: i32, cursor_y: i32 },
    /// Birds-eye view of everything explored so far, pannable independently
    /// of the player - `cam_x`/`cam_y` are the top-left world coordinate.
    Map { cam_x: i32, cam_y: i32 },
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
    visible: &VisibleSet,
) {
    match screen {
        Screen::Look { cursor_x, cursor_y } => {
            draw_look(frame, world, explored, visible, *cursor_x, *cursor_y)
        }
        Screen::Map { cam_x, cam_y } => draw_map_screen(frame, world, explored, *cam_x, *cam_y),
        _ => draw_game(frame, world, camera, explored, visible),
    }
    match screen {
        Screen::Game | Screen::Look { .. } | Screen::Map { .. } => {}
        Screen::Pause { selected } => draw_menu(frame, "Paused", &PAUSE_ITEMS, *selected),
        Screen::Settings { selected } => draw_menu(frame, "Settings", &SETTINGS_ITEMS, *selected),
        Screen::CustomBinds { selected } => draw_custom_binds(frame, binds, *selected),
        Screen::AwaitingKey { direction } => draw_awaiting_key(frame, *direction),
    }
}

/// One visible cell: its glyph, and whether it's in current line-of-sight
/// (full brightness) or only remembered (dimmed).
fn cell(world: &World, explored: &Explored, visible: &VisibleSet, x: i32, y: i32) -> (char, bool) {
    if x == world.player.pos.x && y == world.player.pos.y {
        return ('@', true);
    }
    if visible.is_visible(x, y) {
        let (dx, dy) = (x - world.player.pos.x, y - world.player.pos.y);
        let near = dx * dx + dy * dy <= NEAR_RADIUS * NEAR_RADIUS;
        (world.map.glyph_at(x, y), near)
    } else if explored.is_seen(x, y) {
        (world.map.glyph_at(x, y), false)
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

fn draw_game(frame: &mut Frame, world: &World, camera: &mut Camera, explored: &mut Explored, visible: &VisibleSet) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);
    let (viewport_w, viewport_h) = (area.width as i32, map_height as i32);

    camera.center_on(world.player.pos, viewport_w, viewport_h, world.map.width, world.map.height);

    // Only mark tiles actually within line of sight as explored - a cheap
    // bounding-box pass, distinct from the (cached, more expensive) ray
    // tracing that produced `visible` itself.
    for dy in -SIGHT_RADIUS..=SIGHT_RADIUS {
        for dx in -SIGHT_RADIUS..=SIGHT_RADIUS {
            let (x, y) = (world.player.pos.x + dx, world.player.pos.y + dy);
            if visible.is_visible(x, y) {
                explored.mark(x, y);
            }
        }
    }

    let lines: Vec<Line> = (0..viewport_h)
        .map(|screen_y| {
            let world_y = camera.y + screen_y;
            build_row((0..viewport_w).map(|screen_x| cell(world, explored, visible, camera.x + screen_x, world_y)))
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

/// 'x': free-look cursor. Camera centers on the cursor instead of the
/// player so it can wander beyond the normal viewport - this is how a
/// sight radius larger than the screen becomes actually useful, matching
/// DDA's look-around. Rendering still uses the player's real `visible`/
/// `explored` data (you're looking with your own eyes, not omniscient),
/// movement keys move the cursor instead of stepping the world.
fn draw_look(frame: &mut Frame, world: &World, explored: &Explored, visible: &VisibleSet, cursor_x: i32, cursor_y: i32) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);
    let (viewport_w, viewport_h) = (area.width as i32, map_height as i32);

    let cam_x = (cursor_x - viewport_w / 2).clamp(0, (world.map.width - viewport_w).max(0));
    let cam_y = (cursor_y - viewport_h / 2).clamp(0, (world.map.height - viewport_h).max(0));

    let lines: Vec<Line> = (0..viewport_h)
        .map(|screen_y| {
            let world_y = cam_y + screen_y;
            build_row((0..viewport_w).map(|screen_x| {
                let world_x = cam_x + screen_x;
                if world_x == cursor_x && world_y == cursor_y {
                    ('X', true)
                } else {
                    cell(world, explored, visible, world_x, world_y)
                }
            }))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(0, 0, area.width, map_height),
    );

    let what = describe_tile(world, explored, visible, cursor_x, cursor_y);
    let status = format!("Look: {what} - move to look around, Esc to return");
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(0, map_height, area.width, 1),
    );
}

fn describe_tile(world: &World, explored: &Explored, visible: &VisibleSet, x: i32, y: i32) -> &'static str {
    if x == world.player.pos.x && y == world.player.pos.y {
        return "yourself";
    }
    if !visible.is_visible(x, y) && !explored.is_seen(x, y) {
        return "unknown";
    }
    match world.map.tile(x, y) {
        Some(Tile::Floor) => "floor",
        Some(Tile::Wall) => "a wall",
        Some(Tile::DoorClosed) => "a closed door",
        Some(Tile::DoorOpen) => "an open door",
        Some(Tile::Window { open: true, .. }) => "an open window",
        Some(Tile::Window { open: false, curtain_open: true }) => "a window",
        Some(Tile::Window { open: false, curtain_open: false }) => "a curtained window",
        None => "unknown",
    }
}

/// The 'm' overview: a birds-eye view of everything explored so far,
/// ignoring current line of sight, pannable with the normal movement keys
/// independently of the player. This is a straightforward pannable replay
/// of remembered tiles at 1:1 scale - real DDA's map screen is a separate,
/// much coarser "overmap" layer (one symbol per ~24-tile chunk, showing
/// terrain features like roads/forests/named buildings across a huge area)
/// with its own generation and legend. That's a substantially bigger,
/// separate system - a good candidate to design next, not implemented here.
fn draw_map_screen(frame: &mut Frame, world: &World, explored: &Explored, cam_x: i32, cam_y: i32) {
    let area = frame.area();
    let map_height = area.height.saturating_sub(1);
    let (viewport_w, viewport_h) = (area.width as i32, map_height as i32);

    let lines: Vec<Line> = (0..viewport_h)
        .map(|screen_y| {
            let world_y = cam_y + screen_y;
            let row: String = (0..viewport_w)
                .map(|screen_x| {
                    let world_x = cam_x + screen_x;
                    if world_x == world.player.pos.x && world_y == world.player.pos.y {
                        '@'
                    } else if explored.is_seen(world_x, world_y) {
                        world.map.glyph_at(world_x, world_y)
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
        Paragraph::new("Map - move to look around, Esc to return"),
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
