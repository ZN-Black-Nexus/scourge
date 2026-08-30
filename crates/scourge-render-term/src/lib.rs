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
use scourge_core::{direction_delta, Action, Health, Stats, World};
use scourge_render::{Camera, Explored};
use settings::DisplaySettings;
use ui::Screen;

/// Terminal size as (width, height), with the last row reserved for the status line.
fn viewport_size() -> (i32, i32) {
    terminal::size()
        .map(|(cols, rows)| (cols.max(1) as i32, rows.saturating_sub(1).max(1) as i32))
        .unwrap_or((80, 24))
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

    let mut screen = Screen::NameEntry { name: String::new() };

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
                    ) {
                        return Ok(());
                    }
                    // Checked after every key, not just movement/attack
                    // ones, since a hostile's counterattack can land
                    // during a tick triggered by any turn-costing action
                    // (waiting, picking something up, and so on).
                    if world.player.health.is_dead() && !matches!(screen, Screen::GameOver) {
                        screen = Screen::GameOver;
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

/// Returns false when the game should exit.
fn handle_key(
    code: KeyCode,
    screen: &mut Screen,
    world: &mut World,
    binds: &mut KeyBindings,
    config_path: &std::path::Path,
    display: &mut DisplaySettings,
    settings_path: &std::path::Path,
) -> bool {
    match screen {
        Screen::Game => {
            if code == KeyCode::Esc {
                *screen = Screen::Pause { selected: 0 };
            } else if code == KeyCode::Char('m') {
                let (vw, vh) = viewport_size();
                let cam_x = (world.player.pos.x - vw / 2).clamp(0, (world.map.width - vw).max(0));
                let cam_y = (world.player.pos.y - vh / 2).clamp(0, (world.map.height - vh).max(0));
                *screen = Screen::Map { cam_x, cam_y };
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
            } else if let Some(action) = binds.resolve(code).or_else(|| arrow_action(code)) {
                match action {
                    // UI-only - never reach world.step, just switch screens.
                    Action::OpenWieldMenu => *screen = Screen::WieldSelect,
                    Action::OpenDropMenu => *screen = Screen::DropSelect,
                    Action::OpenExamine => *screen = Screen::ExamineDirection,
                    Action::OpenEatMenu => *screen = Screen::EatSelect,
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
                        *screen = Screen::Examine { x: world.player.pos.x, y: world.player.pos.y };
                    }
                    Action::Move(dir) => {
                        let (dx, dy) = direction_delta(dir);
                        let (x, y) = (world.player.pos.x + dx, world.player.pos.y + dy);
                        *screen = match world.npc_index_at(x, y) {
                            Some(npc_index) => Screen::ExamineNpc { npc_index, selected: 0 },
                            None => Screen::Examine { x, y },
                        };
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
        Screen::Map { cam_x, cam_y } => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            } else if let Some(Action::Move(dir)) = binds.resolve(code).or_else(|| arrow_action(code)) {
                let (dx, dy) = direction_delta(dir);
                let (vw, vh) = viewport_size();
                *cam_x = (*cam_x + dx).clamp(0, (world.map.width - vw).max(0));
                *cam_y = (*cam_y + dy).clamp(0, (world.map.height - vh).max(0));
            }
        }
        Screen::Pause { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::PAUSE_ITEMS.len() - 1)
            }
            KeyCode::Esc => *screen = Screen::Game,
            KeyCode::Enter => match *selected {
                0 => *screen = Screen::Game,
                1 => *screen = Screen::Settings { selected: 0 },
                2 => return false,
                _ => {}
            },
            _ => {}
        },
        Screen::Settings { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(ui::SETTINGS_ITEM_COUNT - 1)
            }
            KeyCode::Esc => *screen = Screen::Pause { selected: 1 },
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
                3 => {
                    display.sidebar_on_left = !display.sidebar_on_left;
                    let _ = display.save(settings_path);
                }
                4 => {
                    display.compass_abbreviate_cardinals = !display.compass_abbreviate_cardinals;
                    let _ = display.save(settings_path);
                }
                _ => {}
            },
            _ => {}
        },
        Screen::CustomBinds { selected } => match code {
            KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(DIRECTIONS.len() - 1),
            KeyCode::Esc => *screen = Screen::Settings { selected: 2 },
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
        let config_path = tmp_path("binds-a");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-a");

        for c in "Riko".chars() {
            handle_key(KeyCode::Char(c), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        }
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert_eq!(world.player.name, "Riko");
        assert!(matches!(screen, Screen::StatAllocation { .. }));

        // Spend 2 points into Strength (selected index 0 by default).
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        handle_key(KeyCode::Right, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        if let Screen::StatAllocation { stats, pool, .. } = &screen {
            assert_eq!(stats.strength, 10);
            assert_eq!(*pool, 2);
        } else {
            panic!("expected StatAllocation");
        }

        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(matches!(screen, Screen::Game));
        assert_eq!(world.player.stats.strength, 10);
        assert_eq!(world.player.health.total().1, Health::from_strength(10).total().1);
    }

    #[test]
    fn empty_name_defaults_to_survivor() {
        let mut world = test_world();
        let mut screen = Screen::NameEntry { name: String::new() };
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-b");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-b");

        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert_eq!(world.player.name, "Survivor");
    }

    #[test]
    fn examine_direction_finds_an_adjacent_npc_and_talk_reaches_world_step() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };
        world.npcs.push(scourge_core::Npc::new("Ally", scourge_core::Pos { x: 6, y: 5 }, scourge_core::Attitude::Friendly));

        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-c");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-c");

        // 'l' is East in the vi preset.
        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(matches!(screen, Screen::ExamineNpc { npc_index: 0, selected: 0 }));

        // Enter on the default selection (Talk) should reach World::step.
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
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
        let config_path = tmp_path("binds-g");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-g");

        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(matches!(screen, Screen::ExamineNpc { npc_index: 0, selected: 0 }));

        // Enter on the default selection (Attack, for a hostile) should
        // reach World::step and defeat it (1 HP left, 10 damage).
        handle_key(KeyCode::Enter, &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
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
        let config_path = tmp_path("binds-h");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-h");

        handle_key(KeyCode::Char('.'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(world.player.health.is_dead());
    }

    #[test]
    fn game_over_screen_quits_on_any_key() {
        let mut world = test_world();
        let mut screen = Screen::GameOver;
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-i");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-i");

        let keep_running = handle_key(KeyCode::Char('x'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(!keep_running);
    }

    #[test]
    fn examine_direction_on_empty_tile_shows_plain_examine() {
        let mut world = test_world();
        world.player.pos = scourge_core::Pos { x: 5, y: 5 };

        let mut screen = Screen::ExamineDirection;
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-d");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-d");

        handle_key(KeyCode::Char('l'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(matches!(screen, Screen::Examine { x: 6, y: 5 }));
    }

    #[test]
    fn sidebar_order_plus_raises_and_minus_lowers_the_selected_section() {
        let mut world = test_world();
        // Start on Hunger (index 1): '+' should raise it above Health.
        let mut screen = Screen::SidebarOrder { selected: 1 };
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-e");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-e");
        let _ = std::fs::remove_file(&settings_path);

        assert_eq!(display.sidebar_order[0], SidebarSection::Health);
        assert_eq!(display.sidebar_order[1], SidebarSection::Hunger);
        handle_key(KeyCode::Char('+'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert_eq!(display.sidebar_order[0], SidebarSection::Hunger);
        assert_eq!(display.sidebar_order[1], SidebarSection::Health);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 0 }));

        let reloaded = DisplaySettings::load(&settings_path).expect("save should have written the file");
        assert_eq!(reloaded.sidebar_order[0], SidebarSection::Hunger);

        // '-' should lower it right back below Health.
        handle_key(KeyCode::Char('-'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert_eq!(display.sidebar_order[0], SidebarSection::Health);
        assert_eq!(display.sidebar_order[1], SidebarSection::Hunger);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 1 }));

        let _ = std::fs::remove_file(&settings_path);
    }

    #[test]
    fn game_screen_opens_sidebar_order_on_curly_brace() {
        let mut world = test_world();
        let mut screen = Screen::Game;
        let mut binds = KeyBindings::preset_vi();
        let config_path = tmp_path("binds-f");
        let mut display = DisplaySettings::default();
        let settings_path = tmp_path("settings-f");

        handle_key(KeyCode::Char('}'), &mut screen, &mut world, &mut binds, &config_path, &mut display, &settings_path);
        assert!(matches!(screen, Screen::SidebarOrder { selected: 0 }));
    }
}
