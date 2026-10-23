# Дорожная карта backspace-rs

Кроссплатформенный клиент Backspace, полностью на Rust.

## Этап 0 — Фундамент (1–2 недели) ✅
- Каркас воркспейса, CI (`cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`).
- `bs-types`: User, Space, Channel, Message, DmChannel, Role, Member, ServerEvent, ClientEvent.
  Тесты десериализации на JSON-фикстурах с реального сервера.
- `bs-permissions`: биты, иерархия, held-bits, версии (FNV-1a 64).
- `bs-api`: reqwest + HttpError с code/details, авторизация Bearer.

## Этап 1 — Один инстанс, только чтение (2–3 недели) ✅
- `bs-ws`: подключение, auth, ready, heartbeat (ping/pong), реконнект с экспоненциальной задержкой.
- `bs-core`: состояние spaces → channels → messages, обработка message_created/updated/deleted, typing, presence_update.
- REST: GET /spaces, GET /spaces/:id, GET /channels/:id/messages (пагинация и учёт forward).
- TUI на ratatui — проверка протокола без GUI.

## Этап 2 — Один инстанс, запись (2–3 недели) ✅
- Отправка сообщений, редактирование, удаление, реакции.
- Read state (channel_ack, mark_unread), unread counts.
- DMs: список, отправка, dm_typing.
- Друзья: запросы, принятие.
- Поиск.

## Этап 3 — Мультиинстанс (3–4 недели) ▶
- `bs-instances`: HashMap<Origin, InstanceHandle>, токены в кейчейне/файле.
- `bs-identity`: UserKey, isMine, myRowIds — порт identity.ts.
- Маршрутизация событий по origin (порт useWebSocket.ts — своя архитектура, не построчная).
- peerHomeWithRemote, ensureRemoteCredential, maybeAutoReattach — по client-federation.md.
- UI: переключатель инстансов, объединённый список spaces/DMs с origin-метками.

## Этап 4 — Голос (отдельный трек, 4+ недель)
- Исследование: webrtc-rs + LiveKit signaling vs FFI к libwebrtc.
- `bs-voice`: подключение к комнате, треки, mute/deafen, screen share (захват — scap/xcap).
- Интеграция с voice_join/leave/status и DM-звонками.

## Этап 5 — Полировка (2–3 недели)
- Уведомления (notify-rust / winrt / macOS). Хоткеи, tray.
- Локализация (i18next-совместимые JSON из locales/).
- Упаковка: AppImage/deb, .dmg, .msi.
