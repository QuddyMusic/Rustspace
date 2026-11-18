//! Эхо-бот: отвечает на `!ping` и повторяет `!echo <текст>`.
//!
//! ```sh
//! RUSTSPACE_URL=https://chat.example.com RUSTSPACE_BOT_TOKEN=rsb_… \
//!     cargo run -p bs-bot --example echo
//! ```

use bs_bot::{Bot, BotError, Context, ServerEvent};

#[tokio::main]
async fn main() -> Result<(), BotError> {
    let url = std::env::var("RUSTSPACE_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let Ok(token) = std::env::var("RUSTSPACE_BOT_TOKEN") else {
        eprintln!("Нужен токен бота в RUSTSPACE_BOT_TOKEN (Настройки → Боты).");
        std::process::exit(2);
    };
    let bot = Bot::new(&url, &token)?;
    tokio::select! {
        result = bot.run(handle) => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

async fn handle(ctx: Context, event: ServerEvent) {
    match event {
        ServerEvent::Ready(ready) => {
            println!(
                "{} на связи, пространств: {}",
                ready.user.username,
                ready.spaces.len()
            );
        }
        ServerEvent::MessageCreated { message } if !ctx.is_me(&message.user) => {
            let text = message.message.content.as_deref().unwrap_or_default();
            let answer = if text == "!ping" {
                "pong".to_owned()
            } else if let Some(rest) = text.strip_prefix("!echo ") {
                rest.to_owned()
            } else {
                return;
            };
            if let Err(err) = ctx.reply(&message, &answer).await {
                eprintln!("не ответил: {err}");
            }
        }
        ServerEvent::DmMessageCreated { message } if !ctx.is_me(&message.user) => {
            let text = message.message.content.clone().unwrap_or_default();
            if let Err(err) = ctx.reply_dm(&message, &text).await {
                eprintln!("не ответил в личке: {err}");
            }
        }
        _ => {}
    }
}
