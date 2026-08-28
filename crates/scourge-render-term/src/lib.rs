//! Terminal rendering backend via crossterm + ratatui, chosen for portability
//! across Windows, macOS, Linux (incl. bare tty), and Termux on Android.

mod keybindings;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEvent};
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

/// A legacy terminal can't tell a genuine second keypress apart from the OS
/// auto-repeating a held key - both arrive as identical `Press` events. DDA
/// requires spamming the key yourself; holding it down doesn't keep moving
/// you. This approximates that: the same key arriving again faster than a
/// deliberate human re-press plausibly could is treated as auto-repeat and
/// dropped, so only the first press of a hold actually steps the world.
const MOVE_REPEAT_THRESHOLD: Duration = Duration::from_millis(150);

struct MoveRepeatGuard {
    last: Option<(KeyCode, Instant)>,
}

impl MoveRepeatGuard {
    fn new() -> Self {
        MoveRepeatGuard { last: None }
    }

    fn allow(&mut self, code: KeyCode) -> bool {
        self.allow_at(code, Instant::now())
    }

    fn allow_at(&mut self, code: KeyCode, now: Instant) -> bool {
        let is_repeat = matches!(self.last, Some((last_code, at)) if last_code == code && now.duration_since(at) < MOVE_REPEAT_THRESHOLD);
        self.last = Some((code, now));
        !is_repeat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_fast_repeats_but_allows_slow_ones_and_other_keys() {
        let mut guard = MoveRepeatGuard::new();
        let t0 = Instant::now();

        assert!(guard.allow_at(KeyCode::Char('h'), t0), "first press always allowed");
        assert!(
            !guard.allow_at(KeyCode::Char('h'), t0 + Duration::from_millis(30)),
            "same key 30ms later looks like OS auto-repeat, not a fresh press"
        );
        assert!(
            guard.allow_at(KeyCode::Char('h'), t0 + Duration::from_millis(300)),
            "same key 300ms later is a deliberate re-press"
        );
        assert!(
            guard.allow_at(KeyCode::Char('l'), t0 + Duration::from_millis(310)),
            "a different key right after is never suppressed"
        );
    }
}

/// Restores the terminal on drop, including on panic, so a crash doesn't leave the user's shell broken.
struct TerminalGuard;

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
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
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
    let mut move_guard = MoveRepeatGuard::new();

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
                    if !handle_key(key.code, &mut screen, &mut world, &mut binds, &config_path, &mut move_guard) {
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
    move_guard: &mut MoveRepeatGuard,
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
                if action == Action::OpenWieldMenu {
                    // UI-only - never reaches world.step, just switches screens.
                    *screen = Screen::WieldSelect;
                } else {
                    // Only movement requires the repeated-press discipline
                    // DDA has; opening doors, picking things up etc. aren't
                    // the "holding a direction" case this guards against.
                    let allowed = !matches!(action, Action::Move(_)) || move_guard.allow(code);
                    if allowed && !world.step(action) {
                        return false;
                    }
                }
            }
        }
        Screen::Inventory => {
            if code == KeyCode::Esc {
                *screen = Screen::Game;
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
