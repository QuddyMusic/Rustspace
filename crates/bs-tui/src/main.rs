//! Точка входа `bs-tui`.

use std::io;
use std::process::ExitCode;
use std::time::Duration;

use bs_tui::app::{App, Screen};
use bs_tui::term::TerminalGuard;
use bs_tui::ui;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Как часто просыпаться, даже если ввода нет: тик для перерисовки и
/// вычерпывания сетевых событий.
const TICK: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("bs-tui: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let (mut guard, mut terminal) = TerminalGuard::enter()?;
    let mut app = App::new();

    while app.is_running() {
        app.pump_network();
        terminal.draw(|frame| ui::render(frame, &app))?;

        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_key(&mut app, key);
        }
    }

    app.logout();
    guard.restore();
    Ok(())
}

/// Единственное место, где клавиши превращаются в действия.
fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit();
        return;
    }

    match app.screen() {
        Screen::Login => handle_login_key(app, key),
        Screen::Chat => handle_chat_key(app, key),
    }
}

fn handle_login_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.quit(),
        KeyCode::Tab => app.form_mut().focus_next(),
        KeyCode::BackTab => app.form_mut().focus_prev(),
        KeyCode::Backspace => app.form_mut().backspace(),
        KeyCode::Enter => {
            app.submit_login();
        }
        KeyCode::Char(c) => app.form_mut().push_char(c),
        _ => {}
    }
}

fn handle_chat_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => app.logout(),
        KeyCode::Char('q') if ctrl => app.quit(),
        KeyCode::Tab | KeyCode::Down => app.nav_channel(1),
        KeyCode::BackTab | KeyCode::Up => app.nav_channel(-1),
        KeyCode::Left => app.nav_space(-1),
        KeyCode::Right => app.nav_space(1),
        KeyCode::Enter => {
            app.send_composer();
        }
        KeyCode::Backspace => app.chat_mut().backspace(),
        KeyCode::Char(c) => {
            app.chat_mut().push_char(c);
            app.notify_typing();
        }
        _ => {}
    }
}
