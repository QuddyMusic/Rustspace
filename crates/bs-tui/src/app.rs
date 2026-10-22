//! Состояние TUI: экраны, форма логина, кэш и подключение.

use bs_core::{Cache, apply};
use bs_instances::ConnectionState;
use bs_types::{ClientEvent, ServerEvent};

use crate::chat::ChatState;
use crate::login::LoginForm;
use crate::net::{NetEvent, NetWorker};

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Ввод адреса, логина и пароля.
    #[default]
    Login,
    /// Список каналов и лента сообщений.
    Chat,
}

/// Состояние подключения с точки зрения интерфейса.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Форма редактируется, сети ещё нет.
    #[default]
    Idle,
    /// Логин в процессе.
    Connecting,
    /// Подключились, события идут.
    Connected,
    /// Ошибка, требующая действия пользователя.
    Failed(String),
}

/// Корневое состояние приложения.
#[derive(Default)]
pub struct App {
    screen: Screen,
    running: bool,
    form: LoginForm,
    status: Status,
    worker: Option<NetWorker>,
    cache: Option<Cache>,
    chat: ChatState,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("running", &self.running)
            .field("form", &self.form)
            .field("status", &self.status)
            .field("worker", &self.worker.is_some())
            .field("cache", &self.cache.is_some())
            .field("chat", &self.chat)
            .finish()
    }
}

impl App {
    /// Создаёт приложение на экране логина.
    pub fn new() -> Self {
        Self {
            screen: Screen::Login,
            running: true,
            form: LoginForm::new(),
            status: Status::Idle,
            worker: None,
            cache: None,
            chat: ChatState::new(),
        }
    }

    /// Текущий экран.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Форма логина.
    pub fn form(&self) -> &LoginForm {
        &self.form
    }

    /// Форма логина для правки.
    pub fn form_mut(&mut self) -> &mut LoginForm {
        &mut self.form
    }

    /// Состояние подключения.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Кэш состояния; `None` до первого `ready`.
    pub fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Выбор канала и композер.
    pub fn chat(&self) -> &ChatState {
        &self.chat
    }

    /// Выбор канала и композер для правки.
    pub fn chat_mut(&mut self) -> &mut ChatState {
        &mut self.chat
    }

    /// Можно ли сейчас начинать логин.
    pub fn can_submit(&self) -> bool {
        self.worker.is_none() && self.form.is_submittable()
    }

    /// Запускает логин в фоне. Возвращает `false`, если это неуместно.
    pub fn submit_login(&mut self) -> bool {
        if !self.can_submit() {
            return false;
        }
        let Some(creds) = self.form.credentials() else {
            return false;
        };
        self.worker = Some(NetWorker::spawn(creds));
        self.status = Status::Connecting;
        true
    }

    /// Забирает накопившиеся события сети и применяет их к состоянию.
    pub fn pump_network(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        for event in worker.drain() {
            self.apply_net_event(event);
        }
    }

    /// Применяет одно событие фоновой сессии.
    pub fn apply_net_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Ready(event) => {
                if let ServerEvent::Ready(payload) = *event {
                    self.cache = Some(Cache::from_ready(*payload));
                }
                self.status = Status::Connected;
                self.screen = Screen::Chat;
                if let Some(cache) = self.cache.as_ref() {
                    self.chat.clamp(cache);
                }
            }
            NetEvent::Server(event) => {
                if let ServerEvent::Ready(_) = event.as_ref() {
                    self.status = Status::Connected;
                    self.screen = Screen::Chat;
                }
                let Some(cache) = self.cache.as_mut() else {
                    return;
                };
                let _ = apply(cache, *event);
                self.chat.clamp(cache);
            }
            NetEvent::State(state) => {
                tracing::debug!(?state, "состояние соединения");
                self.status = match state {
                    ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                        Status::Connecting
                    }
                    ConnectionState::Connected => Status::Connected,
                    ConnectionState::Stopped => Status::Failed("соединение остановлено".to_owned()),
                };
            }
            NetEvent::Failed(message) => {
                self.status = Status::Failed(message);
                self.worker = None;
            }
        }
    }

    /// Отправляет набранный текст в текущий канал.
    ///
    /// Возвращает `false`, если отправлять некуда или нечего.
    pub fn send_composer(&mut self) -> bool {
        let Some(cache) = self.cache.as_ref() else {
            return false;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return false;
        };
        let Some(content) = self.chat.take_composer() else {
            return false;
        };
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        worker.send(ClientEvent::MessageCreate {
            channel_id,
            content,
            reply_to_id: None,
        })
    }

    /// Посылает `typing_start`, если в этом канале ещё не посылали.
    pub fn notify_typing(&mut self) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return;
        };
        if !self.chat.should_send_typing(&channel_id) {
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            worker.send(ClientEvent::TypingStart { channel_id });
        }
    }

    /// Сдвигает выбор канала на `delta` (split borrow: кэш и чат — разные поля).
    pub fn nav_channel(&mut self, delta: isize) {
        let Self {
            cache: Some(cache),
            chat,
            ..
        } = self
        else {
            return;
        };
        chat.move_channel(cache, delta);
    }

    /// Сдвигает выбор пространства на `delta`.
    pub fn nav_space(&mut self, delta: isize) {
        let Self {
            cache: Some(cache),
            chat,
            ..
        } = self
        else {
            return;
        };
        chat.move_space(cache, delta);
    }

    /// Отключается и возвращает на экран логина.
    pub fn logout(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join();
        }
        self.cache = None;
        self.status = Status::Idle;
        self.screen = Screen::Login;
    }

    /// Пора ли выходить из цикла событий.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Просит цикл событий завершиться.
    pub fn quit(&mut self) {
        self.running = false;
    }
}
