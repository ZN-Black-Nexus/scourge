//! Screen state machine and drawing. ratatui diffs frames internally, so
//! only cells that actually changed hit the terminal - no full-screen clear
//! flicker on every move.

use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use scourge_core::{
    compass_direction, format_clock, hp_bar, Action, Attitude, Direction, Skill, Stats, Tile, TileColor, VisibleSet,
    World,
};
use scourge_render::{Camera, Explored};

use crate::keybindings::{describe_key, KeyBindings, DIRECTIONS};
use crate::settings::{DisplaySettings, SidebarSection};

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
/// The Settings list has dynamic items (current sidebar side, compass
/// cardinal style), so its content is built in `draw_settings`; this is
/// just the nav bound.
pub const SETTINGS_ITEM_COUNT: usize = 5;

pub enum Screen {
    Game,
    Inventory,
    /// '@': all 27 skills and their levels - a plain list for now, no
    /// selection/interaction since nothing yet trains or checks them.
    Skills,
    /// 'w'/'h' (preset-dependent): pick an inventory letter to wield, or
    /// '-' to empty your hands.
    WieldSelect,
    /// 'd'/'p': pick an inventory letter to drop onto the player's tile.
    DropSelect,
    /// Shift+E: pick a letter to eat - list only shows food.
    EatSelect,
    /// 'e'/'l': prompts for a direction (or Wait's key for "myself"), then
    /// examines whatever's actually there - real DDA lets you examine any
    /// adjacent tile, not just the one you're standing on.
    ExamineDirection,
    /// A plain tile's description at these world coordinates - dismissed
    /// by any key.
    Examine { x: i32, y: i32 },
    /// The NPC found by `ExamineDirection` - "many more options" is
    /// deliberately just these two for now, since nothing else (combat,
    /// trading, real dialogue) exists yet to hang further options off of.
    ExamineNpc { npc_index: usize, selected: usize },
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
    /// '}': reorders the sidebar's reorderable sections (health/hunger/
    /// stats/wielded/compass) - real BN's own sidebar-layout screen.
    SidebarOrder { selected: usize },
    /// Character creation, step 1: pick a name. The very first screen shown
    /// on a new game.
    NameEntry { name: String },
    /// Character creation, step 2: a small point-buy over the 4 stats -
    /// intentionally just this much, not real DDA's full traits/
    /// professions/scenarios character creator.
    StatAllocation { stats: Stats, pool: i32, selected: usize },
}

pub fn draw(
    frame: &mut Frame,
    world: &World,
    screen: &Screen,
    binds: &KeyBindings,
    display: &DisplaySettings,
    camera: &mut Camera,
    explored: &mut Explored,
    visible: &VisibleSet,
) {
    match screen {
        Screen::Look { cursor_x, cursor_y } => {
            draw_look(frame, world, explored, visible, *cursor_x, *cursor_y)
        }
        Screen::Map { cam_x, cam_y } => draw_map_screen(frame, world, explored, *cam_x, *cam_y),
        Screen::NameEntry { name } => draw_name_entry(frame, name),
        Screen::StatAllocation { stats, pool, selected } => draw_stat_allocation(frame, *stats, *pool, *selected),
        _ => draw_game(frame, world, display, camera, explored, visible),
    }
    match screen {
        Screen::Game | Screen::Look { .. } | Screen::Map { .. } | Screen::NameEntry { .. } | Screen::StatAllocation { .. } => {}
        Screen::Inventory => draw_inventory(frame, world),
        Screen::Skills => draw_skills(frame, world),
        Screen::WieldSelect => draw_wield_select(frame, world),
        Screen::DropSelect => draw_drop_select(frame, world),
        Screen::EatSelect => draw_eat_select(frame, world),
        Screen::ExamineDirection => draw_examine_direction(frame),
        Screen::Examine { x, y } => draw_examine(frame, world, explored, visible, *x, *y),
        Screen::ExamineNpc { npc_index, selected } => draw_examine_npc(frame, world, *npc_index, *selected),
        Screen::Pause { selected } => draw_menu(frame, "Paused", &PAUSE_ITEMS, *selected),
        Screen::Settings { selected } => draw_settings(frame, display, *selected),
        Screen::CustomBinds { selected } => draw_custom_binds(frame, binds, *selected),
        Screen::AwaitingKey { direction } => draw_awaiting_key(frame, *direction),
        Screen::Help => draw_help(frame, binds),
        Screen::SidebarOrder { selected } => draw_sidebar_order(frame, display, *selected),
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
        (ColorSupport::TrueColor, TileColor::Yellow) => Color::Rgb(230, 200, 60),
        (ColorSupport::TrueColor, TileColor::Red) => Color::Rgb(220, 50, 50),
        (ColorSupport::Ansi256, TileColor::White) => Color::Indexed(15),
        (ColorSupport::Ansi256, TileColor::Gray) => Color::Indexed(248),
        (ColorSupport::Ansi256, TileColor::Brown) => Color::Indexed(94),
        (ColorSupport::Ansi256, TileColor::Cyan) => Color::Indexed(87),
        (ColorSupport::Ansi256, TileColor::Green) => Color::Indexed(70),
        (ColorSupport::Ansi256, TileColor::DarkGreen) => Color::Indexed(22),
        (ColorSupport::Ansi256, TileColor::Blue) => Color::Indexed(69),
        (ColorSupport::Ansi256, TileColor::Magenta) => Color::Indexed(170),
        (ColorSupport::Ansi256, TileColor::Yellow) => Color::Indexed(220),
        (ColorSupport::Ansi256, TileColor::Red) => Color::Indexed(196),
        (ColorSupport::Basic, TileColor::White) => Color::White,
        (ColorSupport::Basic, TileColor::Gray) => Color::Gray,
        (ColorSupport::Basic, TileColor::Brown) => Color::Yellow,
        (ColorSupport::Basic, TileColor::Cyan) => Color::Cyan,
        (ColorSupport::Basic, TileColor::Green) => Color::Green,
        (ColorSupport::Basic, TileColor::DarkGreen) => Color::Green,
        (ColorSupport::Basic, TileColor::Blue) => Color::Blue,
        (ColorSupport::Basic, TileColor::Magenta) => Color::Magenta,
        // Distinct from Brown's plain Yellow, even though real DDA doesn't
        // have to make this trade-off - basic 16-color mode is already a
        // lossy fallback tier.
        (ColorSupport::Basic, TileColor::Yellow) => Color::LightYellow,
        (ColorSupport::Basic, TileColor::Red) => Color::Red,
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

/// An NPC renders as the same `@` glyph as the player - it's another
/// survivor, not a monster - distinguished purely by color, matching how
/// DDA tells actors apart on an ASCII display.
fn npc_color() -> Color {
    ratatui_color(TileColor::Yellow)
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
    // NPCs only ever render while actually in sight, never from fog-of-war
    // memory like terrain/items do - they move, so a remembered position
    // would just as often be a lie as the truth.
    if visible.is_visible(x, y) && world.npc_at(x, y).is_some() {
        return ('@', Style::default().fg(npc_color()));
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
fn draw_game(
    frame: &mut Frame,
    world: &World,
    display: &DisplaySettings,
    camera: &mut Camera,
    explored: &mut Explored,
    visible: &VisibleSet,
) {
    let area = frame.area();
    // The constraint *order* determines physical left/right position, so
    // this has to flip which constraint comes first, not just relabel the
    // resulting rects afterward.
    let constraints = if display.sidebar_on_left {
        [Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(20)]
    } else {
        [Constraint::Min(20), Constraint::Length(SIDEBAR_WIDTH)]
    };
    let columns = Layout::default().direction(LayoutDirection::Horizontal).constraints(constraints).split(area);
    let (map_area, sidebar_area) = if display.sidebar_on_left {
        (columns[1], columns[0])
    } else {
        (columns[0], columns[1])
    };

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

    draw_sidebar(frame, world, display, visible, sidebar_area);
}

/// One HP line, e.g. "Head  |||::" by default, or "Head   45/60" with the
/// (not yet reachable) Self-Aware trait - matches real DDA: an idea of your
/// condition, not exact numbers, unless something specific reveals them.
fn hp_line(bp: scourge_core::BodyPart, part: scourge_core::PartHealth, self_aware: bool) -> Line<'static> {
    let ratio = if part.max > 0 { part.current as f64 / part.max as f64 } else { 0.0 };
    let reading = if self_aware {
        format!("{:>3}/{:<3}", part.current, part.max)
    } else {
        hp_bar(part.current, part.max)
    };
    let style = Style::default().fg(hp_color(ratio));
    Line::from(vec![
        Span::raw(format!("{:<6}", bp.label())),
        Span::styled(reading, style),
        Span::raw(if part.is_broken() { " !" } else { "" }),
    ])
}

/// Green-to-red gradient by health ratio, richer when the terminal supports
/// it - an approximation, since DDA's exact color table wasn't available to
/// verify byte-for-byte.
fn hp_color(ratio: f64) -> Color {
    let ratio = ratio.clamp(0.0, 1.0);
    match color_support() {
        ColorSupport::TrueColor => Color::Rgb(((1.0 - ratio) * 220.0) as u8, (ratio * 200.0) as u8, 0),
        _ => {
            if ratio > 0.6 {
                Color::Green
            } else if ratio > 0.3 {
                Color::Yellow
            } else {
                Color::Red
            }
        }
    }
}

/// The compass color for a spotted NPC's name - real BN's own convention:
/// neutral reads as plain gray, hostile as red. Nothing neutral exists yet
/// (only `Attitude::Friendly`/`Hostile`), so friendly gets its own color
/// (green) rather than being lumped in with a "neutral" that doesn't apply.
fn attitude_color(attitude: Attitude) -> Color {
    match attitude {
        Attitude::Friendly => ratatui_color(TileColor::Green),
        Attitude::Hostile => ratatui_color(TileColor::Red),
    }
}

/// Column width for one compass cell (`"North: Ally"` and the like) - a
/// fixed target, not a hard limit, since a longer name just overflows past
/// it and the sidebar's own word-wrap takes over from there. The 8
/// directions always occupy their grid position, present or not, so the
/// compass reads at a glance instead of making you check labels every time
/// something is (or isn't) spotted.
const COMPASS_COL_WIDTH: usize = 9;

/// The 8 diagonals are always abbreviated (NE/NW/SE/SW have no shorter
/// form worth having); the 4 cardinals default to full words but can be
/// abbreviated too via a sidebar setting.
fn compass_label(dir: Direction, abbreviate_cardinals: bool) -> &'static str {
    match dir {
        Direction::North => if abbreviate_cardinals { "N" } else { "North" },
        Direction::South => if abbreviate_cardinals { "S" } else { "South" },
        Direction::East => if abbreviate_cardinals { "E" } else { "East" },
        Direction::West => if abbreviate_cardinals { "W" } else { "West" },
        Direction::NorthEast => "NE",
        Direction::NorthWest => "NW",
        Direction::SouthEast => "SE",
        Direction::SouthWest => "SW",
    }
}

/// The nearest visible NPC in a given compass direction, if any.
fn npc_in_direction<'a>(world: &'a World, visible: &VisibleSet, dir: Direction) -> Option<(&'a str, Color)> {
    world
        .npcs
        .iter()
        .filter(|npc| visible.is_visible(npc.pos.x, npc.pos.y))
        .filter_map(|npc| {
            let (dx, dy) = (npc.pos.x - world.player.pos.x, npc.pos.y - world.player.pos.y);
            if (dx != 0 || dy != 0) && compass_direction(dx, dy) == dir {
                Some((dx * dx + dy * dy, npc))
            } else {
                None
            }
        })
        .min_by_key(|(dist_sq, _)| *dist_sq)
        .map(|(_, npc)| (npc.name.as_str(), attitude_color(npc.attitude)))
}

/// BN's sidebar compass: a fixed 3x3 grid (NW/N/NE over W/·/E over SW/S/SE,
/// player implicitly at the center) naming the nearest visible NPC in each
/// direction, colored by attitude. Always shows all 8 - an empty direction
/// still shows its label, rather than the compass disappearing/reshuffling
/// depending on what's around, which would make it something you have to
/// re-read every time instead of a fixed reference.
fn compass_lines(world: &World, visible: &VisibleSet, abbreviate_cardinals: bool) -> Vec<Line<'static>> {
    // `pad`: the first two columns of a row must hold a fixed width so the
    // next column starts in the same place on every row (that's what makes
    // it read as a grid); a long name there gets truncated to fit rather
    // than pushing everything after it out of line. The third column has
    // nothing after it, so it's left unpadded and untruncated - a long
    // name there can run on without disturbing anything else.
    let cell = |dir: Direction, pad: bool| -> Vec<Span<'static>> {
        let label = compass_label(dir, abbreviate_cardinals);
        let mut spans = vec![Span::raw(format!("{label}:"))];
        let mut used = label.len() + 1;
        if let Some((name, color)) = npc_in_direction(world, visible, dir) {
            // Reserve room for the leading space *and* a guaranteed
            // trailing gap, so a name that fills the whole budget still
            // doesn't run straight into the next column with no separator.
            let name = if pad {
                let budget = COMPASS_COL_WIDTH.saturating_sub(used + 2);
                name.chars().take(budget).collect::<String>()
            } else {
                name.to_string()
            };
            if !name.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(name.clone(), Style::default().fg(color)));
                used += 1 + name.len();
            }
        }
        if pad {
            spans.push(Span::raw(" ".repeat(COMPASS_COL_WIDTH.saturating_sub(used).max(1))));
        }
        spans
    };
    let blank_cell = || vec![Span::raw(" ".repeat(COMPASS_COL_WIDTH))];
    let row = |a: Vec<Span<'static>>, b: Vec<Span<'static>>, c: Vec<Span<'static>>| {
        let mut spans = a;
        spans.extend(b);
        spans.extend(c);
        Line::from(spans)
    };

    vec![
        row(cell(Direction::NorthWest, true), cell(Direction::North, true), cell(Direction::NorthEast, false)),
        row(cell(Direction::West, true), blank_cell(), cell(Direction::East, false)),
        row(cell(Direction::SouthWest, true), cell(Direction::South, true), cell(Direction::SouthEast, false)),
    ]
}

fn section_lines(section: SidebarSection, world: &World) -> Vec<Line<'static>> {
    match section {
        SidebarSection::Health => {
            world.player.health.parts().into_iter().map(|(bp, part)| hp_line(bp, part, world.player.self_aware)).collect()
        }
        SidebarSection::Hunger => vec![Line::raw(format!("Hunger {}/100", world.player.hunger))],
        SidebarSection::Stats => {
            let stats = world.player.stats;
            vec![
                Line::raw(format!("STR {}  DEX {}", stats.strength, stats.dexterity)),
                Line::raw(format!("INT {}  PER {}", stats.intelligence, stats.perception)),
            ]
        }
        SidebarSection::Wielded => vec![Line::raw(match &world.player.wielded {
            Some(item) => format!("Wield: {}", item.name),
            None => "Wield: -".to_string(),
        })],
        SidebarSection::Compass => Vec::new(), // filled in by draw_sidebar, which has `visible`
    }
}

fn draw_sidebar(frame: &mut Frame, world: &World, display: &DisplaySettings, visible: &VisibleSet, area: Rect) {
    let mut lines = vec![Line::raw(format!("{}  ({})", format_clock(world.turn), world.player.name))];
    if let Some(message) = &world.last_message {
        lines.push(Line::styled(message.clone(), Style::default().add_modifier(Modifier::ITALIC)));
    }
    lines.push(Line::raw(""));

    for section in &display.sidebar_order {
        if *section == SidebarSection::Compass {
            lines.extend(compass_lines(world, visible, display.compass_abbreviate_cardinals));
        } else {
            lines.extend(section_lines(*section, world));
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw("? help  @ skills  } layout"));

    let border = if display.sidebar_on_left { Borders::RIGHT } else { Borders::LEFT };
    let block = Block::default().title("Scourge").borders(border);
    frame.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }).block(block), area);
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
    // NPCs are only ever described while actually visible, matching how
    // they're rendered - no fog-of-war memory of where one used to be.
    if !is_player && visible.is_visible(x, y) {
        if let Some(npc) = world.npc_at(x, y) {
            return npc.name.clone();
        }
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

/// '@': all 27 real DDA skills and their levels. Everything starts at 0 -
/// nothing yet trains them (no combat/crafting/social systems exist).
fn draw_skills(frame: &mut Frame, world: &World) {
    let popup = centered_rect(60, 90, frame.area());
    let items: Vec<ListItem> = Skill::ALL
        .iter()
        .map(|&skill| ListItem::new(format!("{:<16}{}", skill.label(), world.player.skills.level(skill))))
        .collect();
    let list = List::new(items).block(Block::default().title("Skills - Esc to close").borders(Borders::ALL));
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

/// 'e'/'l': a description of whatever tile `ExamineDirection` picked -
/// reuses the same description logic as the free-roam 'x' look.
fn draw_examine(frame: &mut Frame, world: &World, explored: &Explored, visible: &VisibleSet, x: i32, y: i32) {
    let popup = centered_rect(60, 20, frame.area());
    let what = describe_tile(world, explored, visible, x, y);
    let text = format!("You see: {what}\n\n(press any key to close)");
    let paragraph = Paragraph::new(text).block(Block::default().title("Examine").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

/// Prompts for which adjacent tile to examine - a movement key picks the
/// direction, Wait's own key examines yourself instead.
fn draw_examine_direction(frame: &mut Frame) {
    let popup = centered_rect(55, 20, frame.area());
    let text = "Examine which direction? (Wait's key for yourself, Esc to cancel)";
    let paragraph = Paragraph::new(text).block(Block::default().title("Examine").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

pub const EXAMINE_NPC_ITEMS: [&str; 3] = ["Talk", "Swap places", "Cancel"];

/// The NPC found by an examine - deliberately just two real actions (Talk,
/// Swap places) rather than a longer menu of things that don't do anything
/// yet.
fn draw_examine_npc(frame: &mut Frame, world: &World, npc_index: usize, selected: usize) {
    let name = world.npcs.get(npc_index).map(|n| n.name.as_str()).unwrap_or("Someone");
    let popup = centered_rect(50, 30, frame.area());
    let list_items: Vec<ListItem> = EXAMINE_NPC_ITEMS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            ListItem::new(*label).style(style)
        })
        .collect();
    let list = List::new(list_items).block(Block::default().title(name).borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
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

/// Same list-with-selection look as `draw_menu`, but some items' labels
/// are dynamic (current sidebar side, compass cardinal style), so it can't
/// be a plain `&[&str]`.
fn draw_settings(frame: &mut Frame, display: &DisplaySettings, selected: usize) {
    let side = if display.sidebar_on_left { "Left" } else { "Right" };
    let cardinals = if display.compass_abbreviate_cardinals { "Abbreviated (N/S/E/W)" } else { "Full words" };
    let items = [
        "Movement: Vi-keys (hjkl + yubn)".to_string(),
        "Movement: WASD (wasd + qezc)".to_string(),
        "Custom keybinds...".to_string(),
        format!("Sidebar: {side} (Enter to switch)"),
        format!("Compass cardinals: {cardinals} (Enter to toggle)"),
    ];
    let popup = centered_rect(50, 40, frame.area());
    let list_items: Vec<ListItem> = items
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            ListItem::new(label).style(style)
        })
        .collect();
    let list = List::new(list_items).block(Block::default().title("Settings").borders(Borders::ALL));
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
        "}: Sidebar layout".to_string(),
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

/// '}': reorders the sidebar's reorderable sections. Up/Down move the
/// cursor, '+' raises and '-' lowers the selected section - deliberately a
/// simpler control scheme than BN's own drag-based one, since a terminal
/// has no mouse-drag to lean on.
fn draw_sidebar_order(frame: &mut Frame, display: &DisplaySettings, selected: usize) {
    let popup = centered_rect(50, 40, frame.area());
    let items: Vec<ListItem> = display
        .sidebar_order
        .iter()
        .enumerate()
        .map(|(i, section)| {
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            ListItem::new(section.label()).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .title("Sidebar Layout (+ raise, - lower, Esc done)")
            .borders(Borders::ALL),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(list, popup);
}

/// Character creation, step 1: pick a name - the very first thing shown on
/// a new game.
fn draw_name_entry(frame: &mut Frame, name: &str) {
    let popup = centered_rect(50, 20, frame.area());
    let text = format!("What's your name?\n\n{name}_\n\n(Enter to confirm)");
    let paragraph = Paragraph::new(text).block(Block::default().title("New Character").borders(Borders::ALL));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

const STAT_LABELS: [&str; 4] = ["Strength", "Dexterity", "Intelligence", "Perception"];

fn stat_value(stats: Stats, index: usize) -> i32 {
    match index {
        0 => stats.strength,
        1 => stats.dexterity,
        2 => stats.intelligence,
        _ => stats.perception,
    }
}

/// Character creation, step 2: a small point-buy, not real DDA's full
/// traits/professions/scenarios character creator - deliberately just
/// enough to make the starting stats a real choice instead of always
/// identical.
fn draw_stat_allocation(frame: &mut Frame, stats: Stats, pool: i32, selected: usize) {
    let popup = centered_rect(50, 40, frame.area());
    let mut lines: Vec<ListItem> = STAT_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if i == selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            ListItem::new(format!("{label:<13}{}", stat_value(stats, i))).style(style)
        })
        .collect();
    lines.push(ListItem::new(""));
    lines.push(ListItem::new(format!("Points remaining: {pool}")));
    lines.push(ListItem::new("Left/Right adjust, Up/Down select, Enter to finish"));
    let list = List::new(lines).block(Block::default().title("Allocate Stats").borders(Borders::ALL));
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

    #[test]
    fn compass_label_defaults_to_full_cardinal_words_but_always_abbreviates_diagonals() {
        assert_eq!(compass_label(Direction::North, false), "North");
        assert_eq!(compass_label(Direction::North, true), "N");
        assert_eq!(compass_label(Direction::NorthEast, false), "NE");
        assert_eq!(compass_label(Direction::NorthEast, true), "NE");
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn compass_lines_always_shows_all_eight_directions_even_when_empty() {
        let world = World::new(scourge_core::Map::new(20, 20, Tile::Floor));
        let visible = world.map.visible_from(world.player.pos, 20);
        let lines = compass_lines(&world, &visible, false);
        assert_eq!(lines.len(), 3, "always 3 rows, present or not");
        assert!(line_text(&lines[0]).contains("NW:"));
        assert!(line_text(&lines[0]).contains("North:"));
        assert!(line_text(&lines[0]).contains("NE:"));
        assert!(line_text(&lines[1]).contains("West:") || line_text(&lines[1]).contains("W:"));
        assert!(line_text(&lines[2]).contains("South:"));
    }

    #[test]
    fn compass_lines_picks_the_nearest_npc_sharing_a_direction() {
        let mut world = World::new(scourge_core::Map::new(20, 20, Tile::Floor));
        world.player.pos = scourge_core::Pos { x: 10, y: 10 };
        world.npcs.push(scourge_core::Npc::new(
            "Far",
            scourge_core::Pos { x: 10, y: 5 },
            scourge_core::Attitude::Friendly,
        ));
        world.npcs.push(scourge_core::Npc::new(
            "Near",
            scourge_core::Pos { x: 10, y: 8 },
            scourge_core::Attitude::Friendly,
        ));
        let visible = world.map.visible_from(world.player.pos, 20);
        // Abbreviated cardinals leave enough room to see the full,
        // untruncated name - truncation itself is covered separately below.
        let lines = compass_lines(&world, &visible, true);
        let north_row = line_text(&lines[0]);
        assert!(north_row.contains("Near"), "closer NPC should win: {north_row}");
        assert!(!north_row.contains("Far"));
    }

    #[test]
    fn compass_lines_never_run_columns_together_even_when_a_name_is_truncated() {
        let mut world = World::new(scourge_core::Map::new(20, 20, Tile::Floor));
        world.player.pos = scourge_core::Pos { x: 10, y: 10 };
        // Full-word cardinal labels leave very little room for a name -
        // this should truncate gracefully, never collide with "NE:".
        world.npcs.push(scourge_core::Npc::new(
            "Alexandria",
            scourge_core::Pos { x: 10, y: 5 },
            scourge_core::Attitude::Friendly,
        ));
        let visible = world.map.visible_from(world.player.pos, 20);
        let lines = compass_lines(&world, &visible, false);
        let north_row = line_text(&lines[0]);
        assert!(north_row.contains("North:"));
        assert!(north_row.contains("NE:"), "third column must still be intact: {north_row}");
        let before_ne = north_row.split("NE:").next().unwrap();
        assert!(before_ne.ends_with(' '), "must not run straight into the next column with no separator: {north_row:?}");
    }
}
