//! Screen state machine and drawing. ratatui diffs frames internally, so
//! only cells that actually changed hit the terminal - no full-screen clear
//! flicker on every move.

use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use scourge_core::{Action, Direction, Tile, TileColor, VisibleSet, World};
use scourge_render::{Camera, Explored};

use crate::keybindings::{describe_key, KeyBindings, DIRECTIONS};

/// Right-side status panel width, DDA/BN's classic default sidebar
/// arrangement instead of a single line of jammed-together info at the
/// bottom of the screen.
const SIDEBAR_WIDTH: u16 = 26;

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
    Inventory,
    /// 'w'/'h' (preset-dependent): pick an inventory letter to wield, or
    /// '-' to empty your hands.
    WieldSelect,
    /// 'd'/'p': pick an inventory letter to drop onto the player's tile.
    DropSelect,
    /// Shift+E: pick a letter to eat - list only shows food.
    EatSelect,
    /// 'e'/'l': a quick, cursor-free description of the player's own tile -
    /// dismissed by any key.
    Examine,
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
    /// '?': a keybind reference, dismissed by any key - built from the
    /// current preset's actual bindings rather than a hardcoded guess.
    Help,
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
        Screen::Inventory => draw_inventory(frame, world),
        Screen::WieldSelect => draw_wield_select(frame, world),
        Screen::DropSelect => draw_drop_select(frame, world),
        Screen::EatSelect => draw_eat_select(frame, world),
        Screen::Examine => draw_examine(frame, world, explored, visible),
        Screen::Pause { selected } => draw_menu(frame, "Paused", &PAUSE_ITEMS, *selected),
        Screen::Settings { selected } => draw_menu(frame, "Settings", &SETTINGS_ITEMS, *selected),
        Screen::CustomBinds { selected } => draw_custom_binds(frame, binds, *selected),
        Screen::AwaitingKey { direction } => draw_awaiting_key(frame, *direction),
        Screen::Help => draw_help(frame, binds),
    }
}

/// How rich a color palette the terminal can actually show, detected once
/// per run (it can't change mid-session) from the environment - the same
/// signals most terminal apps use: `COLORTERM=truecolor`/`24bit` for full
/// RGB (Alacritty, kitty, iTerm2, most modern emulators default to this),
/// `TERM` containing "256color" for the 256-color palette, otherwise the
/// basic ANSI 16 - what a bare Linux tty console is generally limited to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorSupport {
    TrueColor,
    Ansi256,
    Basic,
}

/// Pure classification, kept separate from reading the real environment so
/// it's directly unit-testable without spawning a subprocess.
fn classify_color_support(colorterm: Option<&str>, term: Option<&str>) -> ColorSupport {
    if matches!(colorterm, Some("truecolor") | Some("24bit")) {
        return ColorSupport::TrueColor;
    }
    if term.is_some_and(|t| t.contains("256color")) {
        return ColorSupport::Ansi256;
    }
    ColorSupport::Basic
}

fn color_support() -> ColorSupport {
    static SUPPORT: std::sync::OnceLock<ColorSupport> = std::sync::OnceLock::new();
    *SUPPORT.get_or_init(|| {
        classify_color_support(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    })
}

/// Maps our backend-agnostic terrain color to the richest ratatui
/// representation the terminal actually supports, falling back gracefully.
fn ratatui_color(c: TileColor) -> Color {
    match (color_support(), c) {
        (_, TileColor::Default) => Color::Reset,
        (ColorSupport::TrueColor, TileColor::White) => Color::Rgb(255, 255, 255),
        (ColorSupport::TrueColor, TileColor::Gray) => Color::Rgb(150, 150, 150),
        (ColorSupport::TrueColor, TileColor::Brown) => Color::Rgb(139, 90, 43),
        (ColorSupport::TrueColor, TileColor::Cyan) => Color::Rgb(140, 220, 220),
        (ColorSupport::TrueColor, TileColor::Green) => Color::Rgb(50, 150, 60),
        (ColorSupport::TrueColor, TileColor::DarkGreen) => Color::Rgb(25, 90, 35),
        (ColorSupport::TrueColor, TileColor::Blue) => Color::Rgb(70, 110, 220),
        (ColorSupport::TrueColor, TileColor::Magenta) => Color::Rgb(220, 90, 200),
        (ColorSupport::Ansi256, TileColor::White) => Color::Indexed(15),
        (ColorSupport::Ansi256, TileColor::Gray) => Color::Indexed(248),
        (ColorSupport::Ansi256, TileColor::Brown) => Color::Indexed(94),
        (ColorSupport::Ansi256, TileColor::Cyan) => Color::Indexed(87),
        (ColorSupport::Ansi256, TileColor::Green) => Color::Indexed(70),
        (ColorSupport::Ansi256, TileColor::DarkGreen) => Color::Indexed(22),
        (ColorSupport::Ansi256, TileColor::Blue) => Color::Indexed(69),
        (ColorSupport::Ansi256, TileColor::Magenta) => Color::Indexed(170),
        (ColorSupport::Basic, TileColor::White) => Color::White,
        (ColorSupport::Basic, TileColor::Gray) => Color::Gray,
        (ColorSupport::Basic, TileColor::Brown) => Color::Yellow,
        (ColorSupport::Basic, TileColor::Cyan) => Color::Cyan,
        (ColorSupport::Basic, TileColor::Green) => Color::Green,
        (ColorSupport::Basic, TileColor::DarkGreen) => Color::Green,
        (ColorSupport::Basic, TileColor::Blue) => Color::Blue,
        (ColorSupport::Basic, TileColor::Magenta) => Color::Magenta,
    }
}

/// The marker shown for an item sitting on the ground - distinct from any
/// terrain color so it stands out at a glance.
fn item_marker_color() -> Color {
    ratatui_color(TileColor::Magenta)
}

/// The player glyph's color - white, like real DDA's `@`.
fn player_color() -> Color {
    ratatui_color(TileColor::White)
}

/// One visible cell: its glyph and style. Three tiers, from a single scan of
/// line-of-sight and memory: bright (near, currently visible), dimmed (far
/// but still visible - an approximation of graduated light falloff), and
/// flat gray (remembered but not currently visible - classic roguelike
/// "memory" convention, distinct from the tile's live color on purpose).
fn cell(world: &World, explored: &Explored, visible: &VisibleSet, x: i32, y: i32) -> (char, Style) {
    if x == world.player.pos.x && y == world.player.pos.y {
        return ('@', Style::default().fg(player_color()));
    }
    // A ground item sitting on a tile needs its own marker - otherwise it's
    // invisible on the map, since the cell would just show plain terrain and
    // you'd only ever find out something's there by looking (x) at the exact
    // tile. `?` overrides the terrain glyph but not its brightness tier.
    let has_item = !world.items_at(x, y).is_empty();
    if visible.is_visible(x, y) {
        let (dx, dy) = (x - world.player.pos.x, y - world.player.pos.y);
        let near = dx * dx + dy * dy <= NEAR_RADIUS * NEAR_RADIUS;
        let style = if has_item {
            Style::default().fg(item_marker_color())
        } else {
            Style::default().fg(ratatui_color(world.map.color_at(x, y)))
        };
        let style = if near { style } else { style.add_modifier(Modifier::DIM) };
        let glyph = if has_item { '?' } else { world.map.glyph_at(x, y) };
        (glyph, style)
    } else if explored.is_seen(x, y) {
        let glyph = if has_item { '?' } else { world.map.glyph_at(x, y) };
        (glyph, Style::default().fg(Color::DarkGray))
    } else {
        (' ', Style::default())
    }
}

/// Builds one row as styled spans, grouping consecutive same-style cells so
/// a normal move only touches a handful of runs, not one span per character.
fn build_row(cells: impl Iterator<Item = (char, Style)>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_style = Style::default();
    let mut started = false;
    for (ch, style) in cells {
        if !started {
            current_style = style;
            started = true;
        } else if style != current_style {
            spans.push(Span::styled(std::mem::take(&mut current), current_style));
            current_style = style;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, current_style));
    }
    Line::from(spans)
}

/// DDA/BN's classic default arrangement: a map viewport plus a persistent
/// status sidebar, instead of the map alone with a single crammed line of
/// info at the bottom.
fn draw_game(frame: &mut Frame, world: &World, camera: &mut Camera, explored: &mut Explored, visible: &VisibleSet) {
    let area = frame.area();
    let columns = Layout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(SIDEBAR_WIDTH)])
        .split(area);
    let (map_area, sidebar_area) = (columns[0], columns[1]);

    let (viewport_w, viewport_h) = (map_area.width as i32, map_area.height as i32);
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
    frame.render_widget(Paragraph::new(lines), map_area);

    draw_sidebar(frame, world, sidebar_area);
}

fn draw_sidebar(frame: &mut Frame, world: &World, area: Rect) {
    let (hp_current, hp_max) = world.player.health.total();
    let mut lines = vec![
        Line::raw(format!("Turn {}", world.turn)),
        Line::raw(format!("HP {hp_current}/{hp_max}")),
        Line::raw(""),
    ];
    for (bp, part) in world.player.health.parts() {
        let broken = if part.is_broken() { " !" } else { "" };
        lines.push(Line::raw(format!("{:<6}{:>3}/{:<3}{}", bp.label(), part.current, part.max, broken)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(format!("Hunger {}/100", world.player.hunger)));
    lines.push(Line::raw(""));
    let stats = world.player.stats;
    lines.push(Line::raw(format!("STR {}  DEX {}", stats.strength, stats.dexterity)));
    lines.push(Line::raw(format!("INT {}  PER {}", stats.intelligence, stats.perception)));
    lines.push(Line::raw(""));
    lines.push(Line::raw(match &world.player.wielded {
        Some(item) => format!("Wield: {}", item.name),
        None => "Wield: -".to_string(),
    }));
    lines.push(Line::raw(""));
    lines.push(Line::raw("? for help"));

    let block = Block::default().title("Scourge").borders(Borders::LEFT);
    frame.render_widget(Paragraph::new(lines).block(block), area);
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
                    ('X', Style::default().add_modifier(Modifier::REVERSED))
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

fn describe_tile(world: &World, explored: &Explored, visible: &VisibleSet, x: i32, y: i32) -> String {
    let is_player = x == world.player.pos.x && y == world.player.pos.y;
    if !is_player && !visible.is_visible(x, y) && !explored.is_seen(x, y) {
        return "unknown".to_string();
    }
    let terrain = if is_player {
        "yourself"
    } else {
        match world.map.tile(x, y) {
            Some(Tile::Floor) => "floor",
            Some(Tile::Dirt) => "dirt",
            Some(Tile::Grass) => "grass",
            Some(Tile::TallGrass) => "tall grass",
            Some(Tile::Wall) => "a wall",
            Some(Tile::DoorClosed) => "a closed door",
            Some(Tile::DoorOpen) => "an open door",
            Some(Tile::Window { open: true, .. }) => "an open window",
            Some(Tile::Window { open: false, curtain_open: true }) => "a window",
            Some(Tile::Window { open: false, curtain_open: false }) => "a curtained window",
            Some(Tile::BlueberryBush) => "a blueberry bush",
            Some(Tile::Tree) => "a tree",
            None => "unknown",
        }
    };
    match (world.items_at(x, y).len(), is_player) {
        (0, _) => terrain.to_string(),
        // Picking up only ever acts on the player's own tile, so only hint
        // "g to pick up" when looking at yourself, not some other spot.
        (n, true) => format!("{terrain} ({n} item(s) here, g to pick up)"),
        (n, false) => format!("{terrain} ({n} item(s) here)"),
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
            build_row((0..viewport_w).map(|screen_x| {
                let world_x = cam_x + screen_x;
                if world_x == world.player.pos.x && world_y == world.player.pos.y {
                    ('@', Style::default().fg(player_color()))
                } else if explored.is_seen(world_x, world_y) {
                    if world.items_at(world_x, world_y).is_empty() {
                        (world.map.glyph_at(world_x, world_y), Style::default().fg(Color::DarkGray))
                    } else {
                        ('?', Style::default().fg(item_marker_color()))
                    }
                } else {
                    (' ', Style::default())
                }
            }))
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

/// 'i': a DDA-style lettered item list. Minimal first pass - no weight,
/// volume, categories, or containers yet (nothing to compute those from -
/// items are just names right now); just enough to see what you're
/// carrying, matching the look DDA's inventory screen is going for.
fn inventory_letters(inventory: &[scourge_core::Item]) -> Vec<ListItem<'static>> {
    if inventory.is_empty() {
        return vec![ListItem::new("(nothing carried)")];
    }
    inventory
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let letter = (b'a' + (i as u8 % 26)) as char;
            ListItem::new(format!("{letter} - {}", item.name))
        })
        .collect()
}

/// One line per body part, e.g. "Torso: 60/60" - a broken (0 HP) limb is
/// called out explicitly since that's a real gameplay state (disabled),
/// not just a number.
/// Just items - body health lives permanently in the sidebar now, showing
/// it again here would be redundant.
fn draw_inventory(frame: &mut Frame, world: &World) {
    let popup = centered_rect(70, 70, frame.area());
    let title = format!("Inventory ({} items) - Esc to close", world.player.inventory.len());
    let mut list_items = vec![ListItem::new(match &world.player.wielded {
        Some(item) => format!("Wielding: {}", item.name),
        None => "Wielding: nothing".to_string(),
    })];
    list_items.extend(inventory_letters(&world.player.inventory));
    let list = List::new(list_items).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

/// 'w'/'h': pick a letter to wield, or '-' to empty your hands.
fn draw_wield_select(frame: &mut Frame, world: &World) {
    let popup = centered_rect(70, 70, frame.area());
    let title = "Wield what? (letter to pick, - for empty hands, Esc to cancel)";
    let mut list_items = vec![ListItem::new(match &world.player.wielded {
        Some(item) => format!("Currently wielding: {}", item.name),
        None => "Currently wielding: nothing".to_string(),
    })];
    list_items.extend(inventory_letters(&world.player.inventory));
    let list = List::new(list_items).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

fn draw_drop_select(frame: &mut Frame, world: &World) {
    let popup = centered_rect(70, 70, frame.area());
    let title = "Drop what? (letter to pick, Esc to cancel)";
    let list = List::new(inventory_letters(&world.player.inventory))
        .block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

/// Only lists food - letters still correspond to the item's real inventory
/// index (not renumbered), so pressing one maps back to the right slot.
fn eat_letters(inventory: &[scourge_core::Item]) -> Vec<ListItem<'static>> {
    let food: Vec<_> = inventory.iter().enumerate().filter(|(_, item)| item.is_food()).collect();
    if food.is_empty() {
        return vec![ListItem::new("(nothing edible carried)")];
    }
    food.into_iter()
        .map(|(i, item)| {
            let letter = (b'a' + (i as u8 % 26)) as char;
            ListItem::new(format!("{letter} - {}", item.name))
        })
        .collect()
}

fn draw_eat_select(frame: &mut Frame, world: &World) {
    let popup = centered_rect(70, 70, frame.area());
    let title = format!("Eat what? (hunger: {}/100, letter to pick, Esc to cancel)", world.player.hunger);
    let list =
        List::new(eat_letters(&world.player.inventory)).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

/// 'e'/'l': a quick, cursor-free look at the player's own tile - reuses the
/// same description logic as the free-roam 'x' look.
fn draw_examine(frame: &mut Frame, world: &World, explored: &Explored, visible: &VisibleSet) {
    let popup = centered_rect(60, 20, frame.area());
    let what = describe_tile(world, explored, visible, world.player.pos.x, world.player.pos.y);
    let text = format!("You see: {what}\n\n(press any key to close)");
    let paragraph = Paragraph::new(text).block(Block::default().title("Examine").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
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

/// '?': looks up each action's *actual* key in the current preset rather
/// than hardcoding text, so it stays correct if presets change.
fn draw_help(frame: &mut Frame, binds: &KeyBindings) {
    fn entry(binds: &KeyBindings, label: &str, action: Action) -> String {
        let key = binds.key_for_action(action).map(describe_key).unwrap_or_else(|| "unbound".to_string());
        format!("{label}: {key}")
    }

    let lines = vec![
        "Arrows always move; letter keys depend on your preset (see Settings)".to_string(),
        String::new(),
        entry(binds, "Open", Action::Open),
        entry(binds, "Close", Action::Close),
        entry(binds, "Pick up", Action::PickUp),
        entry(binds, "Wield/hold", Action::OpenWieldMenu),
        entry(binds, "Drop", Action::OpenDropMenu),
        entry(binds, "Examine", Action::OpenExamine),
        entry(binds, "Eat", Action::OpenEatMenu),
        entry(binds, "Wait", Action::Wait),
        String::new(),
        "i: Inventory    m: Map    x: Look around    ?: This help".to_string(),
        "Esc: Pause menu (Settings, Quit)".to_string(),
        String::new(),
        "(press any key to close)".to_string(),
    ];
    let popup = centered_rect(70, 70, frame.area());
    let list = List::new(lines.into_iter().map(ListItem::new).collect::<Vec<_>>())
        .block(Block::default().title("Help").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_support_falls_back_correctly() {
        assert_eq!(classify_color_support(Some("truecolor"), Some("xterm-256color")), ColorSupport::TrueColor);
        assert_eq!(classify_color_support(Some("24bit"), None), ColorSupport::TrueColor);
        assert_eq!(classify_color_support(None, Some("xterm-256color")), ColorSupport::Ansi256);
        assert_eq!(classify_color_support(None, Some("linux")), ColorSupport::Basic, "bare tty console");
        assert_eq!(classify_color_support(None, None), ColorSupport::Basic);
    }
}
