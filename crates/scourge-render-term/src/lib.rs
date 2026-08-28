//! Terminal rendering backend via crossterm + ratatui, chosen for portability
//! across Windows, macOS, Linux (incl. bare tty), and Termux on Android.

mod keybindings;
mod ui;

use std::io;
use std::path::PathBuf;

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
    // when the player actually moves, not on every single draw (e.g. while
    // just navigating a menu).
    let mut visible = world.map.visible_from(world.player.pos, ui::SIGHT_RADIUS);
    let mut last_vis_pos = world.player.pos;

    let mut screen = Screen::Game;

    loop {
        if world.player.pos != last_vis_pos {
            visible = world.map.visible_from(world.player.pos, ui::SIGHT_RADIUS);
            last_vis_pos = world.player.pos;
        }
        terminal.draw(|frame| ui::draw(frame, &world, &screen, &binds, &mut camera, &mut explored, &visible))?;

        match event::read()? {
            // The map is a fixed persistent world now, not regenerated per
            // screen size, so a resize just needs a redraw (next loop turn).
            Event::Resize(_, _) => {}
            // Enabling mouse capture stops terminals from translating wheel
            // scroll into arrow-key escape codes; just ignore mouse input.
            Event::Mouse(MouseEvent { .. }) => {}
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if !handle_key(key.code, &mut screen, &mut world, &mut binds, &config_path) {
                    return Ok(());
                }
            }
            _ => {}
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
            } else if let Some(action) = binds.resolve(code).or_else(|| arrow_action(code)) {
                if !world.step(action) {
                    return false;
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
