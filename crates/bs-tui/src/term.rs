//! Инициализация и восстановление терминала.
//!
//! Терминал приводится в «raw»-режим и переключается на alternate screen.
//! Восстановление делается двумя путями: через [`TerminalGuard`] (обычный
//! выход, в том числе по ошибке) и через panic-hook — иначе паника оставит
//! пользователю сломанный терминал без курсора.

use std::io::{self, Stdout};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Тип терминала ratatui поверх реального stdout.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Владелец «сырого» состояния терминала: восстанавливает его при уничтожении.
pub struct TerminalGuard {
    restored: bool,
}

impl std::fmt::Debug for TerminalGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalGuard")
            .field("restored", &self.restored)
            .finish()
    }
}

impl TerminalGuard {
    /// Включает raw-режим, уводит вывод на alternate screen и прячет курсор.
    ///
    /// При ошибке на любом шаге уже выполненные изменения откатываются.
    pub fn enter() -> io::Result<(Self, Tui)> {
        install_panic_hook();
        enable_raw_mode()?;

        let mut out = io::stdout();
        if let Err(err) = execute!(out, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(err);
        }

        let backend = CrosstermBackend::new(out);
        match Terminal::new(backend) {
            Ok(terminal) => Ok((Self { restored: false }, terminal)),
            Err(err) => {
                let _ = restore_terminal();
                Err(err)
            }
        }
    }

    /// Возвращает терминал в исходное состояние. Идемпотентно.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = restore_terminal();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Снимает raw-режим, возвращает основной экран и показывает курсор.
fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    Ok(())
}

/// Ставит hook, который восстанавливает терминал перед печатью паники.
///
/// Hook ставится один раз: повторные вызовы не накручивают цепочку.
fn install_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            previous(info);
        }));
    });
}
