# backspace-rs

Кроссплатформенный клиент для [Backspace](https://github.com/TheZwiss/backspace), полностью на Rust.

## Статус

Этап 0 завершён, Этап 2 (запись) закрыт: воркспейс собирается целиком, 215 тестов (+5 doc-тестов) зелёные, `clippy --workspace --all-targets -- -D warnings` чист. GUI и TUI умеют логиниться, отправлять/править/удалять сообщения, ставить реакции, подтверждать прочтение, работать с друзьями, заявками, поиском и личными каналами. `bs-voice` — осознанная заглушка до Этапа 4.

## Структура воркспейса

| Крейт | Назначение | Статус |
|---|---|---|
| `bs-types` | Типы данных протокола (User, Space, Channel, Message, события) | ✅ 50 тестов |
| `bs-permissions` | Битовые права, иерархия ролей, override-логика | ✅ 13 тестов |
| `bs-api` | REST-клиент (`/api/*`), Bearer JWT, ошибки с кодами | ✅ 25 интеграционных |
| `bs-ws` | WebSocket-клиент, heartbeat, реконнект, парсинг событий | ✅ 6 тестов |
| `bs-instances` | Мультиинстансность: N соединений, N токенов, федерация | ✅ 10 тестов |
| `bs-identity` | Логин/регистрация, хранение токенов, per-remote credentials | ✅ 7 тестов |
| `bs-voice` | Голос/видео (LiveKit) — отдельный трек | 🚧 заглушка, Этап 4 |
| `bs-core` | Склейка: состояние приложения, стор, команды | ✅ 30 тестов |
| `bs-app` | GUI (`egui`/`eframe`) + точка входа | ✅ 46 тестов, логин, чат, соцэкран |
| `bs-tui` | Терминальный клиент (`ratatui`/`crossterm`) для ручной проверки протокола | ✅ 28 тестов, логин, чат, соцэкран |

## Сборка

```sh
cargo build              # всё
cargo test --workspace   # тесты
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Алиасы: `cargo fc` (fmt --check), `cargo c` (clippy), `cargo t` (тесты), `cargo f` (fmt).
Полный прогон CI — последовательно, одной строкой:

```sh
cargo fc && cargo c && cargo t
```

## Правила

- `unsafe_code = "deny"` на уровне воркспейса. Любой `unsafe` — через явное исключение с обоснованием.
- `.unwrap()`, `.expect()`, `panic!()`, `todo!()` запрещены в продакшн-коде (clippy deny).
- Ошибки: `thiserror` в библиотеках, `anyhow` в приложении.
- Права — `bitflags` над `u64`, сериализация decimal string (JSON теряет биты выше 2^53).
- Snowflake ID — строки, не числа.
- `Origin` — enum `{Home, Remote(Url)}`, не `String` с `''`.

## Лицензия

AGPL-3.0-only, как и оригинальный Backspace.
