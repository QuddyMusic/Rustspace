# Боты

*(bs-server)* Бот — участник инстанса, за которого пишет программа. У него своё имя и токен,
в Rustspace рядом с именем метка **БОТ**, а с сервером он говорит тем же HTTP API и получает
те же события по WebSocket, что и клиенты, — поэтому бота можно написать на любом языке. Для
Rust есть родная библиотека `bs-bot`; на JavaScript или TypeScript (язык оригинального
Backspace) ничего дополнительно не нужно: хватает `fetch` и `WebSocket`. Обёртка для Python
— в планах.

Клиенты оригинального Backspace показывают ботов как обычных участников.

## Как завести бота

1. **Настройки → Для разработчиков**, включите «Режим разработчика», затем **Боты → Создать бота**. Имя для входа (2–32 латинские буквы, цифры, `_`, `.`
   или `-`), отображаемое имя и описание.
2. Скопируйте токен. Он показывается один раз, сервер хранит только его SHA-256. **Новый
   токен** выдаёт другой, старый перестаёт работать.
3. **Добавить в пространство** — только туда, где у вас есть «Управление пространством».
   Убрать бота из пространства — выгнать, как любого участника. Удалённый бот выходит из всех
   пространств.

На одну учётную запись — до 25 ботов.

## Вход

Токен выглядит как `rsb_<id бота>_<секрет>`.

- HTTP: `Authorization: Bot rsb_…` (годится и `Bearer rsb_…`).
- WebSocket `wss://ваш.домен/ws`: первый кадр — `{"type":"auth","token":"rsb_…"}`, сервер
  отвечает `ready` (пользователь бота, его пространства и каналы). Раз в 15 секунд или около
  того шлите `{"type":"ping"}`: молчащие соединения сервер закрывает.

Бот не входит паролем, не меняет учётную запись, не управляет друзьями, двухфакторным входом,
другими ботами и личными мостами и не пользуется API администратора — на такие запросы
ответ `403 bots_not_allowed`.

## Что обычно нужно боту

| Действие | Запрос |
|---|---|
| Написать в канал | `POST /api/channels/:id/messages` `{"content": "…", "replyToId": "…"}` |
| Изменить, удалить | `PATCH /api/messages/:id` `{"content": "…"}`, `DELETE /api/messages/:id` |
| Написать в личку | `POST /api/dm/:id/messages` `{"content": "…"}` |
| Карточки | `POST /api/rustspace/channels/:id/cards` `{"content": "…", "embeds": [{"title": "…"}]}` |
| Реакция | WebSocket `{"type":"reaction_add","messageId":"…","emoji":"👍"}` |
| «Печатает…» | WebSocket `{"type":"typing_start","channelId":"…"}` |

События приходят как `{"type": "…", …}`: `message_created`, `message_updated`,
`message_deleted`, `dm_message_created`, `reaction_added`, `member_joined`, `member_left` и
остальные события оригинала. Свои сообщения бот тоже получает — пропускайте те, где
`user.id` — его собственный. Текст сообщения — до 4000 символов.

## Rust: `bs-bot`

```toml
[dependencies]
bs-bot = { git = "https://github.com/QuddyMusic/Rustspace" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use bs_bot::{Bot, Context, ServerEvent};

#[tokio::main]
async fn main() -> Result<(), bs_bot::BotError> {
    let bot = Bot::new("https://ваш.домен", &std::env::var("RUSTSPACE_BOT_TOKEN").unwrap())?;
    bot.run(|ctx: Context, event: ServerEvent| async move {
        if let ServerEvent::MessageCreated { message } = event {
            if !ctx.is_me(&message.user) && message.message.content.as_deref() == Some("!ping") {
                let _ = ctx.reply(&message, "pong").await;
            }
        }
    })
    .await
}
```

`run` сам переподключается и возвращается, только если сервер не принял токен. Каждое
событие обрабатывается в своей задаче tokio. У `Context` есть `send`, `reply`, `send_dm`,
`reply_dm`, `edit`, `delete`, `react`, `unreact`, `typing`, `emit` для любого события и
`api()` — весь HTTP API. Полный пример — `crates/bs-bot/examples/echo.rs`:

```sh
RUSTSPACE_URL=https://ваш.домен RUSTSPACE_BOT_TOKEN=rsb_… cargo run -p bs-bot --example echo
```

## JavaScript / TypeScript

Node 22 или новее, без пакетов:

```js
const base = "https://ваш.домен";
const token = process.env.RUSTSPACE_BOT_TOKEN;

const ws = new WebSocket(base.replace(/^http/, "ws") + "/ws");
let me;
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "auth", token }));
  setInterval(() => ws.send('{"type":"ping"}'), 15000);
};
ws.onmessage = async ({ data }) => {
  const event = JSON.parse(data);
  if (event.type === "ready") me = event.user;
  if (event.type !== "message_created") return;
  const { message } = event;
  if (message.user.id === me?.id || message.content !== "!ping") return;
  await fetch(`${base}/api/channels/${message.channelId}/messages`, {
    method: "POST",
    headers: { Authorization: `Bot ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ content: "pong", replyToId: message.id }),
  });
};
ws.onclose = () => process.exit(1); // перезапускайте через systemd или pm2
```
