//! Terminal rendering backend via crossterm + ratatui, chosen for portability
//! across Windows, macOS, Linux (incl. bare tty), and Termux on Android.

mod keybindings;
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
use scourge_core::{direction_delta, Action, World};
use scourge_render::{Camera, Explored};
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

    let mut screen = Screen::Game;

    loop {
        if world.turn != last_turn {
            visible = world.map.visible_from(world.player.pos, ui::SIGHT_RADIUS);
            last_turn = world.turn;
        }
        terminal.draw(|frame| ui::draw(frame, &world, &screen, &binds, &mut camera, &mut explored, &visible))?;

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
                    if !handle_key(key.code, &mut screen, &mut world, &mut binds, &config_path) {
                        return Ok(());
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
            } else if let Some(action) = binds.resolve(code).or_else(|| arrow_action(code)) {
                match action {
                    // UI-only - never reach world.step, just switch screens.
                    Action::OpenWieldMenu => *screen = Screen::WieldSelect,
                    Action::OpenDropMenu => *screen = Screen::DropSelect,
                    Action::OpenExamine => *screen = Screen::Examine,
                    _ => {
                        if !world.step(action) {
                            return false;
                        }
                    }
                }
            }
        }
        Screen::Inventory => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
            }
        }
        Screen::Examine => {
            // Dismissed by any key, matching DDA's message-prompt feel.
            *screen = Screen::Game;
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
                *selected = (*selected + 1).min(ui::SETTINGS_ITEMS.len() - 1)
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
    }
    true
}
