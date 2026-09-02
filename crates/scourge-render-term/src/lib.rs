//! Terminal rendering backend via crossterm + ratatui, chosen for portability
//! across Windows, macOS, Linux (incl. bare tty), and Termux on Android.

mod keybindings;
mod settings;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, MouseEvent,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use keybindings::{arrow_action, KeyBindings, DIRECTIONS};
use scourge_core::{direction_delta, Action, Health, Item, Stats, World};
use scourge_render::{Camera, Explored};
use settings::DisplaySettings;
use ui::Screen;

/// Raw terminal size as (width, height) - no reservation for a bottom
/// status line, since the one screen that uses this (the Map screen; see
/// `map_viewport_size`) keeps its hints in the sidebar rather than a
/// separate bottom bar, matching every other screen's own layout.
fn viewport_size() -> (i32, i32) {
    terminal::size().map(|(cols, rows)| (cols.max(1) as i32, rows.max(1) as i32)).unwrap_or((80, 24))
}

/// The Map screen's actual map viewport - `viewport_size` with the
/// sidebar's own width already subtracted, matching what
/// `map_and_sidebar_areas` gives the renderer (the Map screen keeps its
/// sidebar visible; see `draw_map_screen`), so the camera math here
/// agrees with what's actually drawn instead of assuming the full
/// terminal width.
fn map_viewport_size(display: &DisplaySettings) -> (i32, i32) {
    let (total_w, vh) = viewport_size();
    let vw = total_w - ui::sidebar_width(total_w as u16, display.sidebar_width_percent) as i32;
    (vw.max(1), vh)
}

/// Restores the terminal on drop, including on panic, so a crash doesn't leave the user's shell broken.
struct TerminalGuard {
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(
            out,
            EnterAlternateScreen,
            crossterm::event::EnableMouseCapture
        )?;
        out.execute(crossterm::cursor::Hide)?;

        // A legacy terminal reports OS auto-repeat of a held key and a
        // genuine fast re-press as identical events - there's no way to
        // tell them apart, which is exactly why DDA requires physically
        // repeated presses to keep moving. Where the terminal supports the
        // Kitty keyboard protocol (Alacritty, kitty, WezTerm, foot, and
        // others), it reports them distinctly, so we can honor both at
        // once: any number of real presses, however fast, all move you;
        // holding down a key does not.
        let keyboard_enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if keyboard_enhanced {
            execute!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
            )?;
        }

        Ok(TerminalGuard { keyboard_enhanced })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.keyboard_enhanced {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            out,
            crossterm::cursor::Show,
            crossterm::event::DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

pub fn run(mut world: World) -> io::Result<()> {
    let _guard = TerminalGuard::new()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let config_path = PathBuf::from("config/keybindings.json");
    let mut binds = KeyBindings::load(&config_path).unwrap_or_else(|_| KeyBindings::preset_vi());
    let settings_path = PathBuf::from("config/settings.json");
    let mut display = DisplaySettings::load(&settings_path).unwrap_or_default();
    let save_path = PathBuf::from("config/save.json");

    // A save on disk means a survivor already exists - skip character
    // creation and drop straight into their game instead of starting a new
    // one over it. An unreadable/corrupt save is treated the same as no
    // save at all (falls through to a fresh character) rather than erroring
    // out of the whole game.
    let mut screen = Screen::NameEntry { name: String::new() };
    if let Ok(data) = std::fs::read_to_string(&save_path) {
        if let Ok(save) = serde_json::from_str::<scourge_core::SaveData>(&data) {
            world = World::from_save(save);
            screen = Screen::Game;
        }
    }

    let mut camera = Camera::new();
    let mut explored = Explored::new(world.map.width, world.map.height);

    // Line-of-sight ray tracing is the expensive part - only recompute it
    // when something actually happened (a move, or opening/closing a door -
    // anything that changes the world costs a turn), not on every single
    // draw while just navigating a menu. Keyed on the turn counter rather
    // than just player position, since opening a door changes what's
    // visible without moving the player at all - light should flood in the
    // moment the door opens, not only once you've stepped through it.
    let mut visible = world.map.visible_from(world.player.pos, ui::SIGHT_RADIUS);
    let mut last_turn = world.turn;

    loop {
        if world.turn != last_turn {
            visible = world.map.visible_from(world.player.pos, ui::SIGHT_RADIUS);
            last_turn = world.turn;
        }
        terminal.draw(|frame| ui::draw(frame, &world, &screen, &binds, &display, &mut camera, &mut explored, &visible))?;

        // Drain every already-buffered event before rendering again. Holding
        // a key generates OS key-repeat events faster than we can draw; if we
        // rendered once per event, movement would keep "catching up" after
        // release, feeling like the key was stuck. Applying a whole backlog
        // between renders keeps input and display in sync.
        loop {
            match event::read()? {
                Event::Resize(_, _) => {}
                Event::Mouse(MouseEvent { .. }) => {}
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if !handle_key(
                        key.code,
                        &mut screen,
                        &mut world,
                        &mut binds,
                        &config_path,
                        &mut display,
                        &settings_path,
                        &visible,
                        &save_path,
                    ) {
                        return Ok(());
                    }
                    // Checked after every key, not just movement/attack
                    // ones, since a hostile's counterattack can land
                    // during a tick triggered by any turn-costing action
                    // (waiting, picking something up, and so on).
                    if world.player.health.is_dead() && !matches!(screen, Screen::GameOver) {
                        screen = Screen::GameOver;
                        // Permadeath: a dead character's save would just
                        // reload straight back into the death they already
                        // had, forever - clear it so the next launch starts
                        // a fresh character instead.
                        let _ = std::fs::remove_file(&save_path);
                    }
                }
                _ => {}
            }
            if !event::poll(Duration::from_secs(0))? {
                break;
            }
        }
    }
}

/// Best-effort save - if it fails (no write permission, disk full, ...)
/// quitting still works, it just doesn't persist. Nothing else currently
/// depends on a save actually succeeding.
fn save_game(world: &World, save_path: &std::path::Path) {
    if let Some(parent) = save_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&world.to_save()) {
        let _ = std::fs::write(save_path, json);
    }
}

/// What examining tile `(x, y)` should actually open - an NPC (living
/// creature) takes priority, then a corpse's own loot screen instead of
/// just describing it, falling back to the plain description popup for
/// anything else. Shared by both `ExamineDirection` outcomes (a picked
/// direction, or Wait's key meaning "myself").
fn examine_target(world: &World, x: i32, y: i32) -> Screen {
    if let Some(npc_index) = world.npc_index_at(x, y) {
        Screen::ExamineNpc { npc_index, selected: 0 }
    } else if world.items_at(x, y).last().is_some_and(Item::is_corpse) {
        Screen::CorpseLoot { x, y }
    } else {
        Screen::Examine { x, y }
    }
}

/// Returns false when the game should exit.
fn handle_key(
    code: KeyCode,
    screen: &mut Screen,
    world: &mut World,
    binds: &mut KeyBindings,
    config_path: &std::path::Path,
    display: &mut DisplaySettings,
    settings_path: &std::path::Path,
    visible: &scourge_core::VisibleSet,
    save_path: &std::path::Path,
) -> bool {
    match screen {
        Screen::Game => {
            if code == KeyCode::Esc {
                *screen = Screen::Pause { selected: 0 };
            } else if code == KeyCode::Char('m') {
                let (vw, vh) = map_viewport_size(display);
                let cam_x = (world.player.pos.x - vw / 2).clamp(0, (world.map.width - vw).max(0));
                let cam_y = (world.player.pos.y - vh / 2).clamp(0, (world.map.height - vh).max(0));
                *screen = Screen::Map { cam_x, cam_y, zoom: 0 };
            } else if code == KeyCode::Char('x') {
                *screen = Screen::Look {
                    cursor_x: world.player.pos.x,
                    cursor_y: world.player.pos.y,
                };
            } else if code == KeyCode::Char('i') {
                *screen = Screen::Inventory;
            } else if code == KeyCode::Char('?') {
                *screen = Screen::Help;
            } else if code == KeyCode::Char('@') {
                *screen = Screen::Skills;
            } else if code == KeyCode::Char('}') {
                *screen = Screen::SidebarOrder { selected: 0 };
            } else if code == KeyCode::Char('V') {
                // Starts on whichever list actually has something in it -
                // items first, matching real DDA's own priority, falling
                // back to creatures, or an empty item list if neither.
                let mode = if world.visible_items(visible).is_empty() && world.npcs.iter().any(|n| visible.is_visible(n.pos.x, n.pos.y)) {
                    ui::ViewMode::Creatures
                } else {
                    ui::ViewMode::Items
                };
                *screen = Screen::View { mode, index: 0 };
            } else if let Some(action) = binds.resolve(code).or_else(|| arrow_action(code)) {
                match action {
                    // UI-only - never reach world.step, just switch screens.
                    Action::OpenWieldMenu => *screen = Screen::WieldSelect,
                    Action::OpenDropMenu => *screen = Screen::DropSelect,
                    Action::OpenExamine => *screen = Screen::ExamineDirection,
                    Action::OpenEatMenu => *screen = Screen::EatSelect,
                    // A corpse is a loot container, not something plain
                    // `g` should just scoop up whole - open its loot
                    // screen instead of stepping PickUp at all. Anything
                    // else there (or nothing) picks up normally.
                    Action::PickUp
                        if world.pickup_target().is_some_and(|pos| {
                            world.items_at(pos.x, pos.y).last().is_some_and(Item::is_corpse)
                        }) =>
                    {
                        let pos = world.pickup_target().expect("just checked Some above");
                        *screen = Screen::CorpseLoot { x: pos.x, y: pos.y };
                    }
                    _ => {
                        if !world.step(action) {
                            return false;
                        }
                    }
                }
            }
        }
        Screen::Inventory | Screen::Skills => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            }
        }
        Screen::Examine { .. } | Screen::Help => {
            // Dismissed by any key, matching DDA's message-prompt feel.
            *screen = Screen::Game;
        }
        Screen::ExamineDirection => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if let Some(action) = binds.resolve(code).or_else(|| arrow_action(code)) {
                match action {
                    Action::Wait => {
                        let (x, y) = (world.player.pos.x, world.player.pos.y);
                        *screen = examine_target(world, x, y);
                    }
                    Action::Move(dir) => {
                        let (dx, dy) = direction_delta(dir);
                        let (x, y) = (world.player.pos.x + dx, world.player.pos.y + dy);
                        *screen = examine_target(world, x, y);
                    }
                    _ => {}
                }
            }
        }
        Screen::ExamineNpc { npc_index, selected } => {
            let attitude = world.npcs.get(*npc_index).map(|n| n.attitude);
            let item_count = attitude.map(ui::examine_npc_items).map_or(1, <[&str]>::len);
            match code {
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(item_count - 1),
                KeyCode::Esc => *screen = Screen::Game,
                KeyCode::Enter => {
                    match (attitude, *selected) {
                        (Some(scourge_core::Attitude::Friendly), 0) => {
                            world.step(Action::Talk(*npc_index));
                        }
                        (Some(scourge_core::Attitude::Friendly), 1) => {
                            world.step(Action::SwapPlaces(*npc_index));
                        }
                        (Some(scourge_core::Attitude::Hostile), 0) => {
                            world.step(Action::Attack(*npc_index));
                        }
                        _ => {}
                    }
                    *screen = Screen::Game;
                }
                _ => {}
            }
        }
        // Letter-select loot, same convention as WieldSelect/DropSelect -
        // stays open after taking one item so several can be grabbed in a
        // row, only '-' (take the whole corpse) or Esc actually leaves.
        Screen::CorpseLoot { x, y } => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if code == KeyCode::Char('-') {
                world.step(Action::TakeCorpse { x: *x, y: *y });
                *screen = Screen::Game;
            } else if let KeyCode::Char(c) = code {
                if c.is_ascii_lowercase() {
                    let index = (c as u8 - b'a') as usize;
                    world.step(Action::TakeCorpseLoot { x: *x, y: *y, item_index: index });
                }
            }
        }
        Screen::WieldSelect => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if code == KeyCode::Char('-') {
                world.step(Action::Unwield);
                *screen = Screen::Game;
            } else if let KeyCode::Char(c) = code {
                if c.is_ascii_lowercase() {
                    let index = (c as u8 - b'a') as usize;
                    if index < world.player.inventory.len() {
                        world.step(Action::Wield(index));
                        *screen = Screen::Game;
                    }
                }
            }
        }
        Screen::DropSelect => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if let KeyCode::Char(c) = code {
                if c.is_ascii_lowercase() {
                    let index = (c as u8 - b'a') as usize;
                    if index < world.player.inventory.len() {
                        world.step(Action::Drop(index));
                        *screen = Screen::Game;
                    }
                }
            }
        }
        Screen::EatSelect => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if let KeyCode::Char(c) = code {
                if c.is_ascii_lowercase() {
                    let index = (c as u8 - b'a') as usize;
                    if index < world.player.inventory.len() {
                        world.step(Action::Eat(index));
                        *screen = Screen::Game;
                    }
                }
            }
        }
        Screen::Look { cursor_x, cursor_y } => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if let Some(Action::Move(dir)) = binds.resolve(code).or_else(|| arrow_action(code)) {
                let (dx, dy) = direction_delta(dir);
                *cursor_x = (*cursor_x + dx).clamp(0, world.map.width - 1);
                *cursor_y = (*cursor_y + dy).clamp(0, world.map.height - 1);
            }
        }
        Screen::Map { cam_x, cam_y, zoom } => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if code == KeyCode::Char('z') {
                let (vw, vh) = map_viewport_size(display);
                let old_factor = ui::MAP_ZOOM_FACTORS[*zoom];
                *zoom = (*zoom + 1) % ui::MAP_ZOOM_FACTORS.len();
                let new_factor = ui::MAP_ZOOM_FACTORS[*zoom];
                // Recenter on the same world point instead of leaving
                // cam_x/cam_y (the viewport's top-left corner) untouched -
                // otherwise zooming out only grows the view's bottom-right
                // extent, visibly shifting what's on screen.
                let (new_cam_x, new_cam_y) =
                    ui::recenter_for_zoom(*cam_x, *cam_y, old_factor, new_factor, vw, vh, world.map.width, world.map.height);
                *cam_x = new_cam_x;
                *cam_y = new_cam_y;
            } else if let Some(Action::Move(dir)) = binds.resolve(code).or_else(|| arrow_action(code)) {
                let (dx, dy) = direction_delta(dir);
                let (vw, vh) = map_viewport_size(display);
                // Panning steps by a whole block at a time when zoomed
                // out, so a single move feels the same "amount of map" at
                // either zoom level instead of crawling across a now much
                // wider view one tile per press.
                let factor = ui::MAP_ZOOM_FACTORS[*zoom];
                *cam_x = (*cam_x + dx * factor).clamp(0, (world.map.width - vw * factor).max(0));
                *cam_y = (*cam_y + dy * factor).clamp(0, (world.map.height - vh * factor).max(0));
            }
        }
        // Arrow/jk + Enter still works throughout (unchanged), but each
        // item also has a direct letter so a returning player doesn't have
        // to navigate to it every time - matching keys DDA/BN players
        // already have muscle memory for (q to quit, Esc/R to resume).
        Screen::Pause { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::PAUSE_ITEMS.len() - 1)
            }
            KeyCode::Esc | KeyCode::Char('R') => *screen = Screen::Game,
            KeyCode::Char('s') => *screen = Screen::Settings { selected: 0 },
            KeyCode::Char('S') => {
                save_game(world, save_path);
                return false;
            }
            KeyCode::Char('q') => return false,
            KeyCode::Enter => match *selected {
                0 => *screen = Screen::Game,
                1 => *screen = Screen::Settings { selected: 0 },
                2 => {
                    save_game(world, save_path);
                    return false;
                }
                3 => return false,
                _ => {}
            },
            _ => {}
        },
        // The category picker - each category is its own screen below,
        // so this list doesn't grow unreadable as more settings get added.
        Screen::Settings { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::SETTINGS_CATEGORIES.len() - 1)
            }
            KeyCode::Esc => *screen = Screen::Pause { selected: 1 },
            KeyCode::Char('m') => *screen = Screen::MovementSettings { selected: 0 },
            KeyCode::Char('s') => *screen = Screen::SidebarSettings { selected: 0 },
            KeyCode::Enter => match *selected {
                0 => *screen = Screen::MovementSettings { selected: 0 },
                1 => *screen = Screen::SidebarSettings { selected: 0 },
                _ => {}
            },
            _ => {}
        },
        Screen::MovementSettings { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::MOVEMENT_SETTINGS_ITEM_COUNT - 1)
            }
            KeyCode::Esc => *screen = Screen::Settings { selected: 0 },
            KeyCode::Enter => match *selected {
                0 => {
                    *binds = KeyBindings::preset_vi();
                    let _ = binds.save(config_path);
                }
                1 => {
                    *binds = KeyBindings::preset_wasd();
                    let _ = binds.save(config_path);
                }
                2 => *screen = Screen::CustomBinds { selected: 0 },
                _ => {}
            },
            _ => {}
        },
        Screen::SidebarSettings { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::SIDEBAR_SETTINGS_ITEM_COUNT - 1)
            }
            KeyCode::Esc => *screen = Screen::Settings { selected: 1 },
            KeyCode::Enter => match *selected {
                0 => {
                    display.sidebar_on_left = !display.sidebar_on_left;
                    let _ = display.save(settings_path);
                }
                2 => {
                    display.compass_abbreviate_cardinals = !display.compass_abbreviate_cardinals;
                    let _ = display.save(settings_path);
                }
                3 => {
                    display.limbs_two_rows = !display.limbs_two_rows;
                    let _ = display.save(settings_path);
                }
                4 => *screen = Screen::SidebarOrder { selected: 0 },
                _ => {}
            },
            KeyCode::Left if *selected == 1 => {
                display.sidebar_width_percent =
                    display.sidebar_width_percent.saturating_sub(ui::SIDEBAR_WIDTH_STEP).max(ui::MIN_SIDEBAR_PERCENT);
                let _ = display.save(settings_path);
            }
            KeyCode::Right if *selected == 1 => {
                display.sidebar_width_percent =
                    (display.sidebar_width_percent + ui::SIDEBAR_WIDTH_STEP).min(ui::MAX_SIDEBAR_PERCENT);
                let _ = display.save(settings_path);
            }
            _ => {}
        },
        Screen::CustomBinds { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(DIRECTIONS.len() - 1),
            KeyCode::Esc => *screen = Screen::MovementSettings { selected: 2 },
            KeyCode::Enter => {
                *screen = Screen::AwaitingKey {
                    direction: DIRECTIONS[*selected],
                }
            }
            _ => {}
        },
        Screen::AwaitingKey { direction } => {
            let direction = *direction;
            let idx = DIRECTIONS.iter().position(|d| *d == direction).unwrap_or(0);
            if code != KeyCode::Esc {
                binds.bind(code, direction);
                let _ = binds.save(config_path);
            }
            *screen = Screen::CustomBinds { selected: idx };
        }
        Screen::SidebarOrder { selected } => {
            let len = display.sidebar_order.len();
            match code {
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(len - 1),
                // '+' raises the selected section (feels like "boost its
                // priority"), '-' lowers it - the opposite of the first
                // pass, which read backwards once actually used.
                KeyCode::Char('+') | KeyCode::Char('=') if *selected > 0 => {
                    display.sidebar_order.swap(*selected, *selected - 1);
                    *selected -= 1;
                    let _ = display.save(settings_path);
                }
                KeyCode::Char('-') if *selected + 1 < len => {
                    display.sidebar_order.swap(*selected, *selected + 1);
                    *selected += 1;
                    let _ = display.save(settings_path);
                }
                KeyCode::Esc => *screen = Screen::Game,
                _ => {}
            }
        }
        Screen::NameEntry { name } => match code {
            KeyCode::Char(c) if c.is_ascii_graphic() || c == ' ' => {
                if name.len() < 20 {
                    name.push(c);
                }
            }
            KeyCode::Backspace => {
                name.pop();
            }
            KeyCode::Enter => {
                let chosen = if name.trim().is_empty() { "Survivor".to_string() } else { name.clone() };
                world.player.name = chosen;
                *screen = Screen::StatAllocation { stats: Stats::default(), pool: 4, selected: 0 };
            }
            _ => {}
        },
        Screen::StatAllocation { stats, pool, selected } => {
            let get = |s: &Stats, i: usize| match i {
                0 => s.strength,
                1 => s.dexterity,
                2 => s.intelligence,
                _ => s.perception,
            };
            let set = |s: &mut Stats, i: usize, v: i32| match i {
                0 => s.strength = v,
                1 => s.dexterity = v,
                2 => s.intelligence = v,
                _ => s.perception = v,
            };
            match code {
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(3),
                KeyCode::Left | KeyCode::Char('h') => {
                    let current = get(stats, *selected);
                    if current > 4 {
                        set(stats, *selected, current - 1);
                        *pool += 1;
                    }
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    let current = get(stats, *selected);
                    if *pool > 0 && current < 14 {
                        set(stats, *selected, current + 1);
                        *pool -= 1;
                    }
                }
                KeyCode::Enter => {
                    world.player.stats = *stats;
                    world.player.health = Health::from_strength(stats.strength);
                    *screen = Screen::Game;
                }
                KeyCode::Esc => *screen = Screen::NameEntry { name: world.player.name.clone() },
                _ => {}
            }
        }
        Screen::View { mode, index } => match code {
            KeyCode::Esc => *screen = Screen::Game,
            KeyCode::Tab => {
                *mode = match *mode {
                    ui::ViewMode::Items => ui::ViewMode::Creatures,
                    ui::ViewMode::Creatures => ui::ViewMode::Items,
                };
                *index = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                let len = match *mode {
                    ui::ViewMode::Items => world.visible_items(visible).len(),
                    ui::ViewMode::Creatures => world.npcs.iter().filter(|n| visible.is_visible(n.pos.x, n.pos.y)).count(),
                };
                if len > 0 {
                    *index = (*index + 1).min(len - 1);
                }
            }
            _ => {}
        },
        Screen::GameOver => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use scourge_core::{Map, Tile};
    use settings::SidebarSection;

    fn test_world() -> World {
        World::new(Map::new(20, 20, Tile::Floor))
    }

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("scourge-test-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn character_creation_flow_sets_name_and_spent_stats() {
        let mut world = test_world();
        let mut screen = Screen::NameEntry { name: String::new() };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-a");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-a");
        let save_path = tmp_path("save-a");

        for c in "Riko".chars() {
            handle_key(KeyCode::Char(c), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        }
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(world.player.name, "Riko");
        assert!(matches!(screen, Screen::StatAllocation { .. }));

        // Spend 2 points into Strength (selected index 0 by default).
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        if let Screen::StatAllocation { stats, pool, .. } = &screen {
            assert_eq!(stats.strength, 10);
            assert_eq!(*pool, 2);
        } else {
            panic!("expected StatAllocation");
        }

        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Game));
        assert_eq!(world.player.stats.strength, 10);
        assert_eq!(world.player.health.total().1, Health::from_strength(10).total().1);
    }

    #[test]
    fn empty_name_defaults_to_survivor() {
        let mut world = test_world();
        let mut screen = Screen::NameEntry { name: String::new() };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-b");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-b");
        let save_path = tmp_path("save-b");

        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(world.player.name, "Survivor");
    }

    #[test]
    fn examine_direction_finds_an_adjacent_npc_and_talk_reaches_world_step() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };
        world.npcs.push(scourge_core::Npc::new("Ally", scourge_core::Pos { x: 6, y: 5 }, scourge_core::Attitude::Friendly));

        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-c");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-c");
        let save_path = tmp_path("save-c");

        // 'l' is East in the vi preset.
        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::ExamineNpc { npc_index: 0, selected: 0 }));

        // Enter on the default selection (Talk) should reach World::step.
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Game));
        assert_eq!(world.last_message.as_deref(), Some("\"Hey there.\" - Ally"));
    }

    #[test]
    fn examining_a_hostile_npc_offers_attack_instead_of_talk_and_swap() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };
        // Nearly-dead so the follow-up counterattack this same tick doesn't
        // matter for this test - it's just checking the menu wires through.
        let mut zombie = scourge_core::Npc::new("Zombie", scourge_core::Pos { x: 6, y: 5 }, scourge_core::Attitude::Hostile);
        zombie.health.damage(scourge_core::BodyPart::Torso, 59);
        world.npcs.push(zombie);

        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-g");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-g");
        let save_path = tmp_path("save-g");

        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::ExamineNpc { npc_index: 0, selected: 0 }));

        // Enter on the default selection (Attack, for a hostile) should
        // reach World::step and defeat it (1 HP left, 10 damage).
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Game));
        assert_eq!(world.last_message.as_deref(), Some("You defeated the Zombie."));
        assert!(world.npcs.is_empty());
    }

    #[test]
    fn waiting_next_to_a_lethal_hostile_kills_the_player() {
        // handle_key itself never switches to Screen::GameOver - that's
        // run()'s job, checked once per key after handle_key returns (it
        // needs the *outer* loop since death can follow from any
        // turn-costing action, not just this one). This test only confirms
        // the death condition handle_key's caller checks for actually
        // becomes true through the normal action path.
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };
        world.player.health.damage(scourge_core::BodyPart::Torso, 55); // 5 HP left
        world.npcs.push(scourge_core::Npc::new("Zombie", scourge_core::Pos { x: 6, y: 5 }, scourge_core::Attitude::Hostile));

        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-h");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-h");
        let save_path = tmp_path("save-h");

        handle_key(KeyCode::Char('.'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(world.player.health.is_dead());
    }

    #[test]
    fn game_over_screen_quits_on_any_key() {
        let mut world = test_world();
        let mut screen = Screen::GameOver;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-i");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-i");
        let save_path = tmp_path("save-i");

        let keep_running = handle_key(KeyCode::Char('x'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(!keep_running);
    }

    #[test]
    fn shift_v_opens_view_on_items_when_items_are_visible() {
        let mut world = test_world();
        world.place_item(world.player.pos.x, world.player.pos.y, scourge_core::Item::new("Rock"));
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-m");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-m");
        let save_path = tmp_path("save-m");

        handle_key(KeyCode::Char('V'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::View { mode: ui::ViewMode::Items, index: 0 }));
    }

    #[test]
    fn shift_v_falls_back_to_creatures_when_no_items_are_visible() {
        let mut world = test_world();
        world.npcs.push(scourge_core::Npc::new("Ally", world.player.pos, scourge_core::Attitude::Friendly));
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-n");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-n");
        let save_path = tmp_path("save-n");

        handle_key(KeyCode::Char('V'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::View { mode: ui::ViewMode::Creatures, index: 0 }));
    }

    #[test]
    fn view_tab_switches_mode_and_resets_index_and_up_down_clamp() {
        let mut world = test_world();
        world.place_item(world.player.pos.x, world.player.pos.y, scourge_core::Item::new("Rock"));
        world.npcs.push(scourge_core::Npc::new("Ally", world.player.pos, scourge_core::Attitude::Friendly));
        let mut screen = Screen::View { mode: ui::ViewMode::Items, index: 0 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-o");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-o");
        let save_path = tmp_path("save-o");

        // Only one item, so Down shouldn't move past index 0.
        handle_key(KeyCode::Down, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::View { mode: ui::ViewMode::Items, index: 0 }));

        handle_key(KeyCode::Tab, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::View { mode: ui::ViewMode::Creatures, index: 0 }));

        handle_key(KeyCode::Esc, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Game));
    }

    #[test]
    fn examine_direction_on_empty_tile_shows_plain_examine() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };

        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-d");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-d");
        let save_path = tmp_path("save-d");

        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Examine { x: 6, y: 5 }));
    }

    #[test]
    fn sidebar_order_plus_raises_and_minus_lowers_the_selected_section() {
        let mut world = test_world();
        // Start on Hunger (index 1): '+' should raise it above Health.
        let mut screen = Screen::SidebarOrder { selected: 1 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-e");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-e");
        let save_path = tmp_path("save-e");
        let _ = std::fs::remove_file(&settings_path);

        assert_eq!(display.sidebar_order[0], SidebarSection::Health);
        assert_eq!(display.sidebar_order[1], SidebarSection::Hunger);
        handle_key(KeyCode::Char('+'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_order[0], SidebarSection::Hunger);
        assert_eq!(display.sidebar_order[1], SidebarSection::Health);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 0 }));

        let reloaded = DisplaySettings::load(&settings_path).expect("save should have written the file");
        assert_eq!(reloaded.sidebar_order[0], SidebarSection::Hunger);

        // '-' should lower it right back below Health.
        handle_key(KeyCode::Char('-'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_order[0], SidebarSection::Health);
        assert_eq!(display.sidebar_order[1], SidebarSection::Hunger);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 1 }));

        let _ = std::fs::remove_file(&settings_path);
    }

    #[test]
    fn settings_screen_adjusts_sidebar_width_with_left_right_and_clamps() {
        let mut world = test_world();
        let mut screen = Screen::SidebarSettings { selected: 1 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-j");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-j");
        let save_path = tmp_path("save-j");
        let _ = std::fs::remove_file(&settings_path);

        assert_eq!(display.sidebar_width_percent, 33, "defaults to real C:BN's own ~1/3-of-screen sidebar");
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_width_percent, 38);
        handle_key(KeyCode::Left, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        handle_key(KeyCode::Left, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_width_percent, 28);

        // Clamp at the top.
        display.sidebar_width_percent = ui::MAX_SIDEBAR_PERCENT;
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_width_percent, ui::MAX_SIDEBAR_PERCENT);

        // Clamp at the bottom.
        display.sidebar_width_percent = ui::MIN_SIDEBAR_PERCENT;
        handle_key(KeyCode::Left, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(display.sidebar_width_percent, ui::MIN_SIDEBAR_PERCENT);

        let _ = std::fs::remove_file(&settings_path);
    }

    #[test]
    fn settings_screen_toggles_limb_layout() {
        let mut world = test_world();
        let mut screen = Screen::SidebarSettings { selected: 3 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-k");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-k");
        let save_path = tmp_path("save-k");
        let _ = std::fs::remove_file(&settings_path);

        assert!(display.limbs_two_rows);
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(!display.limbs_two_rows);

        let _ = std::fs::remove_file(&settings_path);
    }

    #[test]
    fn settings_screen_opens_the_right_category_and_esc_returns_to_it() {
        let mut world = test_world();
        let mut screen = Screen::Settings { selected: 0 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-l");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-l");
        let save_path = tmp_path("save-l");

        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::MovementSettings { selected: 0 }));
        handle_key(KeyCode::Esc, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Settings { selected: 0 }));

        handle_key(KeyCode::Down, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::SidebarSettings { selected: 0 }));
        handle_key(KeyCode::Esc, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Settings { selected: 1 }));
    }

    #[test]
    fn game_screen_opens_sidebar_order_on_curly_brace() {
        let mut world = test_world();
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-f");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-f");
        let save_path = tmp_path("save-f");

        handle_key(KeyCode::Char('}'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 0 }));
    }

    #[test]
    fn pause_menu_direct_keys_bypass_arrow_navigation() {
        let mut world = test_world();
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-p");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-p");
        let save_path = tmp_path("save-p");
        let _ = std::fs::remove_file(&save_path);

        // 'R' resumes regardless of which item is currently selected.
        let mut screen = Screen::Pause { selected: 3 };
        handle_key(KeyCode::Char('R'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Game));

        // 's' opens Settings directly.
        let mut screen = Screen::Pause { selected: 0 };
        handle_key(KeyCode::Char('s'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Settings { selected: 0 }));

        // 'q' quits without writing a save file.
        let mut screen = Screen::Pause { selected: 0 };
        let keep_running = handle_key(KeyCode::Char('q'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(!keep_running, "'q' should quit");
        assert!(!save_path.exists(), "plain quit should not leave a save behind");

        let _ = std::fs::remove_file(&save_path);
    }

    #[test]
    fn pause_menu_save_and_quit_writes_a_reloadable_save() {
        let mut world = test_world();
        world.player.name = "Runner".to_string();
        world.turn = 42;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-q");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-q");
        let save_path = tmp_path("save-q");
        let _ = std::fs::remove_file(&save_path);

        let mut screen = Screen::Pause { selected: 0 };
        let keep_running = handle_key(KeyCode::Char('S'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(!keep_running, "'S' should quit after saving");

        let data = std::fs::read_to_string(&save_path).expect("Save and Quit should have written a file");
        let save: scourge_core::SaveData = serde_json::from_str(&data).expect("save file should be valid JSON");
        assert_eq!(save.player.name, "Runner");
        assert_eq!(save.turn, 42);

        let _ = std::fs::remove_file(&save_path);
    }

    #[test]
    fn settings_category_direct_keys_open_the_matching_screen() {
        let mut world = test_world();
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-r");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-r");
        let save_path = tmp_path("save-r");

        let mut screen = Screen::Settings { selected: 1 };
        handle_key(KeyCode::Char('m'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::MovementSettings { selected: 0 }));

        let mut screen = Screen::Settings { selected: 0 };
        handle_key(KeyCode::Char('s'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::SidebarSettings { selected: 0 }));
    }

    #[test]
    fn map_screen_opens_at_zoom_zero_and_z_toggles_between_the_two_levels() {
        let mut world = test_world();
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-s2");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-s2");
        let save_path = tmp_path("save-s2");

        handle_key(KeyCode::Char('m'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Map { zoom: 0, .. }), "opens zoomed in by default");

        handle_key(KeyCode::Char('z'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Map { zoom: 1, .. }));

        handle_key(KeyCode::Char('z'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::Map { zoom: 0, .. }), "toggles back with only two levels");
    }

    #[test]
    fn map_screen_panning_speed_scales_with_zoom_factor() {
        // Big enough that even a wide test-fallback viewport (`viewport_size`
        // has no real terminal to query here) times the 4x zoom factor
        // still fits comfortably inside the map, so the pan isn't clamped
        // back to 0 by simply running out of map to show.
        let mut world = World::new(Map::new(2000, 2000, Tile::Floor));
        let mut screen = Screen::Map { cam_x: 1000, cam_y: 1000, zoom: 0 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-t2");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-t2");
        let save_path = tmp_path("save-t2");

        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        match screen {
            Screen::Map { cam_x, .. } => assert_eq!(cam_x, 1001, "one tile per press at 1x zoom"),
            _ => panic!("expected Screen::Map"),
        }

        // Toggling zoom recenters the view (see `map_screen_zoom_toggle_
        // recenters_instead_of_shifting_the_view`), so don't assert an
        // exact cam_x here - just that panning afterward still moves by
        // the new zoom factor.
        handle_key(KeyCode::Char('z'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        let cam_x_after_zoom = match screen {
            Screen::Map { cam_x, .. } => cam_x,
            _ => panic!("expected Screen::Map"),
        };
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        match screen {
            Screen::Map { cam_x, .. } => assert_eq!(cam_x - cam_x_after_zoom, 4, "a whole 4-tile block per press at 4x zoom"),
            _ => panic!("expected Screen::Map"),
        }
    }

    #[test]
    fn pickup_near_a_corpse_opens_its_loot_screen_instead_of_grabbing_it() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 0, y: 0 };
        world.place_item(0, 0, Item::corpse("Zombie corpse", scourge_core::TileColor::Gray, vec![]));
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-u");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-u");
        let save_path = tmp_path("save-u");

        handle_key(KeyCode::Char('g'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::CorpseLoot { x: 0, y: 0 }));
        assert!(world.player.inventory.is_empty(), "the corpse itself shouldn't be auto-picked-up");
    }

    #[test]
    fn examine_direction_toward_a_corpse_opens_its_loot_screen() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };
        world.place_item(6, 5, Item::corpse("Zombie corpse", scourge_core::TileColor::Gray, vec![]));
        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-v");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-v");
        let save_path = tmp_path("save-v");

        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert!(matches!(screen, Screen::CorpseLoot { x: 6, y: 5 }));
    }

    #[test]
    fn corpse_loot_screen_letter_takes_loot_and_dash_takes_the_whole_corpse() {
        let mut world = test_world();
        world.place_item(0, 0, Item::corpse("Zombie corpse", scourge_core::TileColor::Gray, vec![Item::new("Rock")]));
        let mut screen = Screen::CorpseLoot { x: 0, y: 0 };
        let mut binds = KeyBindings::preset_vi();
        let visible = world.map.visible_from(world.player.pos, 20);
        let config_path = tmp_path("binds-w");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-w");
        let save_path = tmp_path("save-w");

        handle_key(KeyCode::Char('a'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(world.player.inventory, vec![Item::new("Rock")], "took the loot");
        assert!(matches!(screen, Screen::CorpseLoot { .. }), "stays open to keep looting");

        handle_key(KeyCode::Char('-'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path, &visible, &save_path);
        assert_eq!(world.player.inventory.len(), 2, "now also carrying the emptied-out corpse itself");
        assert!(matches!(screen, Screen::Game), "taking the whole corpse leaves the screen");
        assert!(world.items_at(0, 0).is_empty());
    }
}
