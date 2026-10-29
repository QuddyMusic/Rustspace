//! Отрисовка кадра.

use bs_types::Origin;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::app::{App, Screen, Status};
use crate::chat;
use crate::login::Field;
use crate::social::{self, SocialState, SocialTab};

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
        Paragraph::new(header_line(app))
            .alignment(Alignment::Center)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL)),
        header,
    );

    match app.screen() {
        Screen::Login => render_login(frame, app, body),
        Screen::Chat => render_chat(frame, app, body),
        Screen::Social => render_social(frame, app, body),
    }

    frame.render_widget(
        Paragraph::new(Line::from(footer_hint(app))).alignment(Alignment::Center),
        footer,
    );
}

/// Метка инстанса для интерфейса: `home` или адрес удалённого.
fn instance_label(origin: &Origin) -> String {
    if origin.is_home() {
        "home".to_owned()
    } else {
        origin.as_web_str()
    }
}

/// Верхняя строка: название и подключённый инстанс (он всегда один).
fn header_line(app: &App) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw("backspace-rs TUI")];
    if app.has_instances() {
        spans.push(Span::raw(" — "));
        spans.push(Span::styled(
            instance_label(app.active()),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    Line::from(spans)
}

/// Заголовок ленты: канал.
fn chat_title(channel: Option<&str>) -> Line<'static> {
    Line::from(Span::raw(
        channel.map_or_else(|| " Чат ".to_owned(), |name| format!(" #{name} ")),
    ))
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

    render_sidebar(frame, app, sidebar_area);

    let channel_id = app.chat().current_channel_id(cache);
    let channel_name = channel_id
        .as_deref()
        .and_then(|id| cache.channel(id))
        .map(|c| c.name.as_str());
    let title = chat_title(channel_name);

    let me = cache.me().id.as_str();
    let rows = channel_id.as_deref().map_or_else(Vec::new, |id| {
        chat::message_rows(cache, id, MESSAGE_WINDOW, me)
    });
    let selected = app.chat().selected_message_id();
    let lines: Vec<Line<'_>> = rows
        .iter()
        .map(|row| {
            let mut spans = vec![
                Span::styled(row.author.clone(), Style::default().fg(Color::Magenta)),
                Span::raw(": "),
                Span::raw(row.text.clone()),
            ];
            if row.edited {
                spans.push(Span::styled(
                    " (изменено)",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let line = Line::from(spans);
            if selected == Some(row.id.as_str()) {
                line.style(Style::default().bg(Color::DarkGray))
            } else {
                line
            }
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

    let composer_title = if app.chat().is_editing() {
        " Правка (Esc — отмена) "
    } else {
        " Сообщение "
    };
    let composer_style = if app.chat().is_editing() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(app.chat().composer())
            .style(composer_style)
            .block(Block::default().borders(Borders::ALL).title(composer_title)),
        composer_area,
    );
}

fn render_sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let spaces = chat::combined_sidebar(app.multi());
    let mut items: Vec<ListItem<'_>> = Vec::new();
    for (space_index, space) in spaces.iter().enumerate() {
        let space_style = if space_index == app.chat().space_index() {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::styled(space.name.clone(), space_style)];
        if &space.origin != app.active() {
            spans.push(Span::styled(
                format!(" @{}", instance_label(&space.origin)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        items.push(ListItem::new(Line::from(spans)));
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

fn render_social(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let [tabs_area, body, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(area);

    let social = app.social();
    let tabs: Vec<Span<'_>> = [
        SocialTab::Friends,
        SocialTab::Requests,
        SocialTab::Search,
        SocialTab::Dms,
    ]
    .into_iter()
    .map(|tab| {
        let title = format!(" {} ", tab.title());
        if tab == social.tab() {
            Span::styled(
                title,
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(title)
        }
    })
    .collect();
    frame.render_widget(Paragraph::new(Line::from(tabs)), tabs_area);

    let Some(cache) = app.cache() else {
        frame.render_widget(
            Paragraph::new("Состояние ещё не загружено")
                .block(Block::default().borders(Borders::ALL).title(" Социальное ")),
            body,
        );
        return;
    };

    let cursor = social.cursor();
    let items = social_items(cache, social);

    let title = format!(" {} ", social.tab().title());
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");
    let mut state = ratatui::widgets::ListState::default();
    if social.len(cache) > 0 {
        state.select(Some(cursor));
    }
    frame.render_stateful_widget(list, body, &mut state);

    let mut hints: Vec<Line<'_>> = Vec::new();
    if social.tab() == SocialTab::Search {
        hints.push(Line::from(vec![
            Span::raw("Поиск: "),
            Span::styled(social.query(), Style::default().fg(Color::Cyan)),
        ]));
    }
    if let Some(message) = social.message() {
        hints.push(Line::from(Span::styled(
            message.to_owned(),
            Style::default().fg(Color::Yellow),
        )));
    }
    frame.render_widget(Paragraph::new(hints), hint_area);
}

/// Строки активного списка социального экрана.
fn social_items<'a>(cache: &bs_core::Cache, social: &'a SocialState) -> Vec<ListItem<'a>> {
    match social.tab() {
        SocialTab::Friends => social::friend_rows(cache)
            .into_iter()
            .map(|row| {
                ListItem::new(Line::from(vec![
                    Span::styled(row.name, Style::default().fg(Color::Magenta)),
                    Span::raw(" — "),
                    Span::styled(row.status, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect(),
        SocialTab::Requests => social::request_rows(cache)
            .into_iter()
            .map(|row| {
                let mark = if row.incoming {
                    "входящая"
                } else {
                    "исходящая"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(row.name, Style::default().fg(Color::Magenta)),
                    Span::raw(" — "),
                    Span::styled(mark, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect(),
        SocialTab::Search => social::user_rows(cache, social.search_results())
            .into_iter()
            .map(|row| {
                let mark = if row.friend {
                    " (в друзьях)"
                } else {
                    ""
                };
                ListItem::new(Line::from(vec![
                    Span::styled(row.name, Style::default().fg(Color::Magenta)),
                    Span::styled(mark, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect(),
        SocialTab::Dms => social::dm_rows(cache)
            .into_iter()
            .map(|row| {
                let last = row.last.unwrap_or_default();
                ListItem::new(Line::from(vec![
                    Span::styled(row.name, Style::default().fg(Color::Cyan)),
                    Span::raw(" — "),
                    Span::styled(last, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect(),
    }
}

fn footer_hint(app: &App) -> String {
    match app.screen() {
        Screen::Login => {
            format!(
                "Tab — поле, Enter — войти, Esc — выход | {}",
                app.form().hint()
            )
        }
        Screen::Chat => {
            "Tab — канал, ↑↓ — сообщение, e — правка, d — удалить, +/− — реакция, a — прочитано, Esc — назад | Ctrl+w — выйти из инстанса".to_owned()
        }
        Screen::Social => {
            "←→ — вкладка, ↑↓ — выбор, Enter — открыть/принять, x — удалить, Esc — назад".to_owned()
        }
    }
}
