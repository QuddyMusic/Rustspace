//! Точка входа `bs-tui`.

use std::io;
use std::process::ExitCode;
use std::time::Duration;

use bs_tui::app::{App, Screen};
use bs_tui::chat::{self, ChatCommand};
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
        Screen::Social => handle_social_key(app, key),
    }
}

fn handle_social_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let typing = app.social().tab() == bs_tui::social::SocialTab::Search;
    match key.code {
        KeyCode::Esc => app.close_social(),
        KeyCode::Char('q') if ctrl => app.quit(),
        KeyCode::Left => app.social_prev_tab(),
        KeyCode::Right => app.social_next_tab(),
        KeyCode::Down => app.social_move(1),
        KeyCode::Up => app.social_move(-1),
        KeyCode::Enter => {
            if typing {
                app.run_search();
            } else {
                app.social_open_dm();
            }
        }
        KeyCode::Backspace => {
            if typing {
                app.social_backspace();
            }
        }
        KeyCode::Char('r') if !typing => {
            app.load_tab();
        }
        KeyCode::Char('a') if !typing => {
            app.social_accept_request();
        }
        KeyCode::Char('d') if !typing => {
            app.social_decline_request();
        }
        KeyCode::Char('x') if !typing => {
            if !app.social_cancel_request() {
                app.social_remove_friend();
            }
        }
        KeyCode::Char('n') if !typing => {
            app.social_send_request();
        }
        KeyCode::Char(c) if typing => app.social_push_char(c),
        _ => {}
    }
}

fn handle_login_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            if !app.cancel_login() {
                app.quit();
            }
        }
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

/// Клавиши чата: навигация, композер и Ctrl-сочетания.
///
/// Ctrl-сочетания: `Q` — выход, `S` — соцэкран, `]`/`[` — следующий и
/// предыдущий инстанс, `I` — форма добавления инстанса, `W` — отключиться
/// от активного инстанса.
fn handle_chat_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => {
            if app.chat().is_editing() {
                app.cancel_edit();
            } else {
                app.logout();
            }
        }
        KeyCode::Char('q') if ctrl => app.quit(),
        KeyCode::Tab => app.nav_channel(1),
        KeyCode::BackTab => app.nav_channel(-1),
        KeyCode::Down => app.nav_message(1),
        KeyCode::Up => app.nav_message(-1),
        KeyCode::Left => app.nav_space(-1),
        KeyCode::Right => app.nav_space(1),
        KeyCode::Enter => {
            app.send_composer();
        }
        KeyCode::Backspace => app.chat_mut().backspace(),
        KeyCode::Char('s') if ctrl => app.open_social(),
        KeyCode::Char(']') if ctrl => app.next_instance(),
        KeyCode::Char('[') if ctrl => app.prev_instance(),
        KeyCode::Char('i') if ctrl => app.begin_add_instance(),
        KeyCode::Char('w') if ctrl => app.disconnect_active(),
        KeyCode::Char(c) => handle_chat_char(app, c),
        _ => {}
    }
}

/// Одиночная клавиша в чате: команда или символ в композер.
///
/// Команды срабатывают только при пустом композере и вне режима правки,
/// иначе `e`, `d`, `+`, `-` и `a` — обычный текст сообщения.
fn handle_chat_char(app: &mut App, c: char) {
    let plain = app.chat().composer().is_empty() && !app.chat().is_editing();
    if plain && let Some(command) = chat::chat_command(c) {
        match command {
            ChatCommand::Edit => {
                app.edit_selected();
            }
            ChatCommand::Delete => {
                app.delete_selected();
            }
            ChatCommand::React => {
                app.react_selected(chat::DEFAULT_REACTION);
            }
            ChatCommand::Unreact => {
                app.unreact_selected(chat::DEFAULT_REACTION);
            }
            ChatCommand::Ack => {
                app.ack_current_channel();
            }
            ChatCommand::Mute => {
                app.toggle_mute();
            }
            ChatCommand::Deafen => {
                app.toggle_deafen();
            }
        }
        return;
    }
    app.chat_mut().push_char(c);
    app.notify_typing();
}
