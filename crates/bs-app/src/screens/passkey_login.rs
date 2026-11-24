//! Вход по ключу доступа (passkey): билет у инстанса, подтверждение в
//! браузере (Bitwarden, телефон, Windows Hello…), опрос, вход по токену.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use bs_types::TransportPolicy;

/// Что приходит из фоновой задачи.
#[derive(Debug)]
enum Event {
    /// Билет готов: открыть страницу, показать код.
    Ticket { code: String, url: String },
    /// Вход выполнен.
    Token(String),
    /// Не получилось.
    Failed(String),
}

/// Идущий вход по ключу.
#[derive(Debug)]
pub(crate) struct PasskeyLogin {
    pub(crate) base: String,
    pub(crate) transport: TransportPolicy,
    /// Код, который должен совпасть со страницей в браузере.
    pub(crate) code: Option<String>,
    pub(crate) error: Option<String>,
    rx: Receiver<Event>,
}

/// Чем закончился кадр.
pub(crate) enum Step {
    /// Ждём.
    Waiting,
    /// Есть токен — входить.
    Token(String),
}

impl PasskeyLogin {
    /// Начинает вход: билет, браузер, опрос до пяти минут.
    pub(crate) fn start(
        runtime: &tokio::runtime::Handle,
        base: String,
        transport: TransportPolicy,
    ) -> Self {
        let (tx, rx) = channel();
        let client =
            bs_api::ApiClient::with_policy(bs_types::Origin::Home, base.clone(), transport);
        runtime.spawn(async move {
            let client = match client {
                Ok(client) => client,
                Err(err) => {
                    let _ = tx.send(Event::Failed(err.to_string()));
                    return;
                }
            };
            let ticket = match client.passkey_login_ticket().await {
                Ok(ticket) => ticket,
                Err(err) => {
                    let message = if err.status() == Some(reqwest::StatusCode::NOT_FOUND)
                        || err.code() == Some("passkeys_unavailable")
                    {
                        crate::i18n::tr("Этот инстанс не поддерживает вход по ключу доступа")
                            .to_owned()
                    } else {
                        err.to_string()
                    };
                    let _ = tx.send(Event::Failed(message));
                    return;
                }
            };
            let _ = tx.send(Event::Ticket {
                code: ticket.code,
                url: ticket.url,
            });
            for _ in 0..150 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                match client.passkey_status(&ticket.ticket).await {
                    Ok(bs_api::PasskeyStatus::Pending) => {}
                    Ok(bs_api::PasskeyStatus::Done { token: Some(token) }) => {
                        let _ = tx.send(Event::Token(token));
                        return;
                    }
                    Ok(bs_api::PasskeyStatus::Failed(reason)) => {
                        let _ = tx.send(Event::Failed(reason));
                        return;
                    }
                    Ok(bs_api::PasskeyStatus::Done { token: None }) | Err(_) => {
                        let _ = tx.send(Event::Failed(
                            crate::i18n::tr("Время на вход вышло — начните заново").to_owned(),
                        ));
                        return;
                    }
                }
            }
            let _ = tx.send(Event::Failed(
                crate::i18n::tr("Время на вход вышло — начните заново").to_owned(),
            ));
        });
        Self {
            base,
            transport,
            code: None,
            error: None,
            rx,
        }
    }

    /// Забирает события; открывает страницу в браузере, когда билет готов.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) -> Step {
        ctx.request_repaint_after(Duration::from_millis(500));
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Ticket { code, url } => {
                    if let Err(err) = opener::open(&url) {
                        self.error =
                            Some(crate::tf!("Не удалось открыть браузер: {err}", err = err));
                    }
                    self.code = Some(code);
                }
                Event::Token(token) => return Step::Token(token),
                Event::Failed(message) => self.error = Some(message),
            }
        }
        Step::Waiting
    }
}
