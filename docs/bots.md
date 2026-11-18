# Bots

*(bs-server)* A bot is a member of the instance whose messages come from a program. It has
its own name and token, shows a **BOT** badge in Rustspace, and uses the same HTTP API and
WebSocket events as the clients do — so a bot can be written in any language. Rust has a
native library, `bs-bot`; for JavaScript or TypeScript (the language of the original
Backspace) nothing extra is needed: `fetch` and `WebSocket` are enough. A Python wrapper is
planned.

Clients of the original Backspace show bots as ordinary members.

## Creating a bot

1. **Settings → Bots → Create bot.** Pick a username (2–32 Latin letters, digits, `_`, `.`
   or `-`), a display name and a description.
2. Copy the token. It is shown once; the server keeps only its SHA-256. **New token** issues
   another one and the old one stops working.
3. **Add to space** — only spaces where you have "Manage space". To remove a bot from a
   space, kick it like any member. Deleting the bot removes it from every space.

One account may own up to 25 bots.

## Signing in

The token looks like `rsb_<bot id>_<secret>`.

- HTTP: `Authorization: Bot rsb_…` (`Bearer rsb_…` works too).
- WebSocket `wss://your.domain/ws`: the first frame is `{"type":"auth","token":"rsb_…"}`, the
  server answers with `ready` (the bot's user, its spaces and channels). Send
  `{"type":"ping"}` every 15 seconds or so; the server drops silent connections.

A bot cannot sign in with a password, change the account, manage friends, two-factor,
other bots or personal bridges, or use the admin API — those requests get `403
bots_not_allowed`.

## What a bot usually needs

| Action | Request |
|---|---|
| Send to a channel | `POST /api/channels/:id/messages` `{"content": "…", "replyToId": "…"}` |
| Edit, delete | `PATCH /api/messages/:id` `{"content": "…"}`, `DELETE /api/messages/:id` |
| Send to a DM | `POST /api/dm/:id/messages` `{"content": "…"}` |
| Cards (embeds) | `POST /api/rustspace/channels/:id/cards` `{"content": "…", "embeds": [{"title": "…"}]}` |
| React | WebSocket `{"type":"reaction_add","messageId":"…","emoji":"👍"}` |
| "Typing…" | WebSocket `{"type":"typing_start","channelId":"…"}` |

Events arrive as `{"type": "…", …}`: `message_created`, `message_updated`,
`message_deleted`, `dm_message_created`, `reaction_added`, `member_joined`, `member_left` and
the rest of the original's gateway. The bot also receives its own messages — skip those whose
`user.id` is the bot's. Message text is at most 4000 characters.

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
    let bot = Bot::new("https://your.domain", &std::env::var("RUSTSPACE_BOT_TOKEN").unwrap())?;
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

`run` reconnects on its own and returns only if the server rejects the token. Each event is
handled in its own tokio task. `Context` has `send`, `reply`, `send_dm`, `reply_dm`, `edit`,
`delete`, `react`, `unreact`, `typing`, `emit` for any gateway event, and `api()` for the
whole HTTP API. A complete example: `crates/bs-bot/examples/echo.rs`:

```sh
RUSTSPACE_URL=https://your.domain RUSTSPACE_BOT_TOKEN=rsb_… cargo run -p bs-bot --example echo
```

## JavaScript / TypeScript

Node 22 or newer, no packages:

```js
const base = "https://your.domain";
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
ws.onclose = () => process.exit(1); // restart it with systemd or pm2
```
