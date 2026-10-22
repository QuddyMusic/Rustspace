//! Отрисовка кадра.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::app::{App, Screen, Status};
use crate::chat::{self, sidebar};
use crate::login::Field;

/// Сколько последних сообщений показывать в ленте.
const MESSAGE_WINDOW: usize = 500;

/// Рисует один кадр по текущему состоянию приложения.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("backspace-rs TUI")
            .alignment(Alignment::Center)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL)),
        header,
    );

    match app.screen() {
        Screen::Login => render_login(frame, app, body),
        Screen::Chat => render_chat(frame, app, body),
    }

    frame.render_widget(
        Paragraph::new(Line::from(footer_hint(app))).alignment(Alignment::Center),
        footer,
    );
}

fn render_login(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Вход ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [fields, status] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(2)]).areas(inner);

    let form = app.form();
    let mut lines = Vec::new();
    for field in [Field::Base, Field::Username, Field::Password] {
        let focused = form.focus() == field;
        let marker = if focused { "> " } else { "  " };
        let style = if focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{}: ", field.label()), style),
            Span::styled(form.display_value(field), style.fg(Color::Cyan)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), fields);

    let status_line = match app.status() {
        Status::Idle => Line::from(""),
        Status::Connecting => Line::from(Span::styled(
            "Подключение…",
            Style::default().fg(Color::Yellow),
        )),
        Status::Connected => Line::from(Span::styled(
            "Подключено",
            Style::default().fg(Color::Green),
        )),
        Status::Failed(message) => Line::from(Span::styled(
            format!("Ошибка: {message}"),
            Style::default().fg(Color::Red),
        )),
    };
    frame.render_widget(Paragraph::new(status_line), status);

    // Курсор — в конце активного поля.
    let row = match form.focus() {
        Field::Base => 0,
        Field::Username => 1,
        Field::Password => 2,
    };
    let prefix = format!("> {}: ", form.focus().label()).chars().count();
    let value_len = form.value(form.focus()).chars().count();
    let cursor_x = fields.x + u16::try_from(prefix + value_len).unwrap_or(u16::MAX);
    let cursor_y = fields.y + row;
    frame.set_cursor_position(Position::new(
        cursor_x.min(fields.right().saturating_sub(1)),
        cursor_y.min(fields.bottom().saturating_sub(1)),
    ));
}

fn render_chat(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let [sidebar_area, main] =
        Layout::horizontal([Constraint::Length(28), Constraint::Min(20)]).areas(area);
    let [messages_area, typing_area, composer_area] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(main);

    let Some(cache) = app.cache() else {
        frame.render_widget(
            Paragraph::new("Состояние ещё не загружено")
                .block(Block::default().borders(Borders::ALL).title(" Чат ")),
            main,
        );
        return;
    };

    render_sidebar(frame, app, cache, sidebar_area);

    let channel_id = app.chat().current_channel_id(cache);
    let title = channel_id
        .as_deref()
        .and_then(|id| cache.channel(id))
        .map_or_else(|| " Чат ".to_owned(), |c| format!(" #{} ", c.name));

    let rows = channel_id
        .as_deref()
        .map_or_else(Vec::new, |id| chat::message_rows(cache, id, MESSAGE_WINDOW));
    let lines: Vec<Line<'_>> = rows
        .iter()
        .map(|(author, text)| {
            Line::from(vec![
                Span::styled(author.clone(), Style::default().fg(Color::Magenta)),
                Span::raw(": "),
                Span::raw(text.clone()),
            ])
        })
        .collect();

    let block = Block::default().borders(Borders::ALL).title(title);
    let inner_height = usize::from(block.inner(messages_area).height);
    let skip = lines.len().saturating_sub(inner_height);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>()).block(block),
        messages_area,
    );

    let typing = channel_id
        .as_deref()
        .map_or_else(Vec::new, |id| chat::typing_names(cache, id));
    let typing_line = if typing.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(
            format!("печатает: {}", typing.join(", ")),
            Style::default().fg(Color::DarkGray),
        ))
    };
    frame.render_widget(Paragraph::new(typing_line), typing_area);

    frame.render_widget(
        Paragraph::new(app.chat().composer())
            .block(Block::default().borders(Borders::ALL).title(" Сообщение ")),
        composer_area,
    );
}

fn render_sidebar(frame: &mut Frame<'_>, app: &App, cache: &bs_core::Cache, area: Rect) {
    let spaces = sidebar(cache);
    let mut items: Vec<ListItem<'_>> = Vec::new();
    for (space_index, space) in spaces.iter().enumerate() {
        let space_style = if space_index == app.chat().space_index() {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        items.push(ListItem::new(Line::from(Span::styled(
            space.name.clone(),
            space_style,
        ))));
        if space_index != app.chat().space_index() {
            continue;
        }
        for (channel_index, channel) in space.channels.iter().enumerate() {
            let selected = channel_index == app.chat().channel_index();
            let mut style = if selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            if channel.unread > 0 {
                style = style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
            }
            let marker = if channel.unread > 0 { "• " } else { "  " };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {marker}#{}", channel.name),
                style,
            ))));
        }
    }

    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Каналы ")),
        area,
    );
}

fn footer_hint(app: &App) -> String {
    match app.screen() {
        Screen::Login => format!(
            "Tab — поле, Enter — войти, Esc — выход | {}",
            app.form().hint()
        ),
        Screen::Chat => "Tab — канал, Enter — отправить, Esc — разлогиниться, q — выход".to_owned(),
    }
}
