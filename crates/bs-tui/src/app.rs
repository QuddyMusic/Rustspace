//! Состояние TUI: экраны, форма логина, кэш и подключение.

use bs_core::{Cache, apply};
use bs_instances::ConnectionState;
use bs_types::{ClientEvent, ServerEvent};

use crate::chat::ChatState;
use crate::login::LoginForm;
use crate::net::{NetEvent, NetRequest, NetResponse, NetWorker};
use crate::social::{self, SocialSelection, SocialState, SocialTab};

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Ввод адреса, логина и пароля.
    #[default]
    Login,
    /// Список каналов и лента сообщений.
    Chat,
    /// Друзья, заявки, поиск и личные каналы.
    Social,
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
    social: SocialState,
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
            .field("social", &self.social)
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
            social: SocialState::new(),
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

    /// Состояние социального экрана.
    pub fn social(&self) -> &SocialState {
        &self.social
    }

    /// Состояние социального экрана для правки.
    pub fn social_mut(&mut self) -> &mut SocialState {
        &mut self.social
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
        let events = worker.drain();
        let responses = worker.drain_responses();
        for event in events {
            self.apply_net_event(event);
        }
        for response in responses {
            self.apply_net_response(response);
        }
    }

    /// Применяет ответ фоновой сессии на REST-запрос.
    pub fn apply_net_response(&mut self, response: NetResponse) {
        match response {
            NetResponse::Friends(_)
            | NetResponse::FriendRequests(_)
            | NetResponse::DmChannels(_) => {
                if let Some(cache) = self.cache.as_ref() {
                    self.social.clamp(cache);
                }
            }
            NetResponse::Users(users) => {
                self.social.set_search_results(users);
                self.social.set_message(None);
            }
            NetResponse::DmChannel(channel) => {
                self.social
                    .set_message(Some(format!("личный канал: {}", channel.id)));
            }
            NetResponse::Done => {
                self.social.set_message(None);
                self.refresh_social();
            }
            NetResponse::Failed(message) => {
                self.social.set_message(Some(message));
            }
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
                    self.social.clamp(cache);
                }
                self.refresh_social();
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
                self.social.clamp(cache);
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
    /// В режиме правки вместо нового сообщения уходит `message_edit`.
    /// Возвращает `false`, если отправлять некуда или нечего.
    pub fn send_composer(&mut self) -> bool {
        if self.cache.is_none() || self.worker.is_none() {
            return false;
        }
        let editing = self.chat.editing_message_id().map(str::to_owned);
        let Some(content) = self.chat.take_composer() else {
            return false;
        };
        let event = if let Some(message_id) = editing {
            self.chat.finish_edit();
            ClientEvent::MessageEdit {
                message_id,
                content,
            }
        } else {
            let Some(channel_id) = self.current_channel_id() else {
                return false;
            };
            ClientEvent::MessageCreate {
                channel_id,
                content,
                reply_to_id: None,
            }
        };
        self.send_event(event)
    }

    /// id текущего канала по кэшу.
    fn current_channel_id(&self) -> Option<String> {
        self.cache
            .as_ref()
            .and_then(|cache| self.chat.current_channel_id(cache))
    }

    /// Отправляет событие в фоновую сессию.
    fn send_event(&self, event: ClientEvent) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(event))
    }

    /// Сдвигает курсор по ленте сообщений текущего канала.
    pub fn nav_message(&mut self, delta: isize) {
        let Self {
            cache: Some(cache),
            chat,
            ..
        } = self
        else {
            return;
        };
        chat.move_selection(cache, delta);
    }

    /// Начинает правку выбранного своего сообщения.
    ///
    /// Возвращает `false`, если сообщение не выбрано или написано не нами.
    pub fn edit_selected(&mut self) -> bool {
        let Some(cache) = self.cache.as_ref() else {
            return false;
        };
        let Some(message) = self.chat.selected_message(cache) else {
            return false;
        };
        if message.user.id != cache.me().id {
            return false;
        }
        let message_id = message.message.id.clone();
        let content = message.message.content.clone().unwrap_or_default();
        self.chat.begin_edit(message_id, content);
        true
    }

    /// Отменяет режим правки, если он активен.
    pub fn cancel_edit(&mut self) {
        self.chat.cancel_edit();
    }

    /// Удаляет выбранное своё сообщение.
    ///
    /// Возвращает `false`, если сообщение не выбрано или написано не нами.
    pub fn delete_selected(&mut self) -> bool {
        let Some(cache) = self.cache.as_ref() else {
            return false;
        };
        let Some(message) = self.chat.selected_message(cache) else {
            return false;
        };
        if message.user.id != cache.me().id {
            return false;
        }
        let message_id = message.message.id.clone();
        if self.chat.editing_message_id() == Some(message_id.as_str()) {
            self.chat.cancel_edit();
        }
        self.send_event(ClientEvent::MessageDelete { message_id })
    }

    /// Ставит реакцию на выбранное сообщение.
    pub fn react_selected(&mut self, emoji: &str) -> bool {
        let Some(message_id) = self.selected_message_id() else {
            return false;
        };
        self.send_event(ClientEvent::ReactionAdd {
            message_id,
            emoji: emoji.to_owned(),
        })
    }

    /// Убирает реакцию с выбранного сообщения.
    pub fn unreact_selected(&mut self, emoji: &str) -> bool {
        let Some(message_id) = self.selected_message_id() else {
            return false;
        };
        self.send_event(ClientEvent::ReactionRemove {
            message_id,
            emoji: emoji.to_owned(),
        })
    }

    /// id выбранного сообщения в текущем канале.
    fn selected_message_id(&self) -> Option<String> {
        let cache = self.cache.as_ref()?;
        self.chat
            .selected_message(cache)
            .map(|message| message.message.id.clone())
    }

    /// Подтверждает прочтение текущего канала до последнего сообщения.
    ///
    /// Повторный вызов для той же пары «канал, сообщение» ничего не шлёт.
    pub fn ack_current_channel(&mut self) -> bool {
        let Some(cache) = self.cache.as_ref() else {
            return false;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return false;
        };
        let Some(last) = cache.messages(&channel_id).last() else {
            return false;
        };
        let message_id = last.message.id.clone();
        if !self.chat.should_ack(&channel_id, &message_id) {
            return false;
        }
        self.send_event(ClientEvent::ChannelAck {
            channel_id,
            message_id,
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
        {
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
        self.ack_current_channel();
    }

    /// Сдвигает выбор пространства на `delta`.
    pub fn nav_space(&mut self, delta: isize) {
        {
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
        self.ack_current_channel();
    }

    /// Отправляет REST-запрос фоновой сессии.
    fn request(&self, request: NetRequest) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.request(request))
    }

    /// Перезапрашивает друзей и заявки.
    fn refresh_social(&self) {
        let _ = self.request(NetRequest::Friends);
        let _ = self.request(NetRequest::FriendRequests);
    }

    /// Открывает социальный экран и подгружает данные вкладки.
    pub fn open_social(&mut self) {
        self.screen = Screen::Social;
        self.load_tab();
    }

    /// Возвращает с чата на социальный экран.
    pub fn close_social(&mut self) {
        self.screen = Screen::Chat;
    }

    /// Запрашивает данные для активной вкладки.
    pub fn load_tab(&self) {
        match self.social.tab() {
            SocialTab::Friends => {
                let _ = self.request(NetRequest::Friends);
            }
            SocialTab::Requests => {
                let _ = self.request(NetRequest::FriendRequests);
            }
            SocialTab::Dms => {
                let _ = self.request(NetRequest::DmChannels);
            }
            SocialTab::Search => self.run_search(),
        }
    }

    /// Переключает вкладку на следующую и подгружает её данные.
    pub fn social_next_tab(&mut self) {
        self.social.next_tab();
        self.load_tab();
    }

    /// Переключает вкладку на предыдущую и подгружает её данные.
    pub fn social_prev_tab(&mut self) {
        self.social.prev_tab();
        self.load_tab();
    }

    /// Сдвигает курсор активного списка.
    pub fn social_move(&mut self, delta: isize) {
        let Self {
            cache: Some(cache),
            social,
            ..
        } = self
        else {
            return;
        };
        social.move_cursor(cache, delta);
    }

    /// Запускает поиск по текущей строке запроса.
    pub fn run_search(&self) {
        let query = self.social.query().trim();
        if query.is_empty() {
            return;
        }
        let _ = self.request(NetRequest::SearchUsers(query.to_owned()));
    }

    /// Ввод символа в строку поиска.
    pub fn social_push_char(&mut self, c: char) {
        self.social.push_query(c);
    }

    /// Удаление символа из строки поиска.
    pub fn social_backspace(&mut self) {
        self.social.backspace_query();
    }

    /// Что сейчас выбрано на социальном экране.
    ///
    /// Возвращает пару «вид элемента, его id» — этого хватает, чтобы
    /// отличить действие с другом от действия с заявкой.
    pub fn social_selection(&self) -> Option<SocialSelection> {
        let cache = self.cache.as_ref()?;
        let cursor = self.social.cursor();
        match self.social.tab() {
            SocialTab::Friends => social::friend_rows(cache)
                .get(cursor)
                .map(|row| SocialSelection::Friend(row.id.clone())),
            SocialTab::Requests => social::request_rows(cache).get(cursor).map(|row| {
                if row.incoming {
                    SocialSelection::IncomingRequest(row.id.clone())
                } else {
                    SocialSelection::OutgoingRequest(row.id.clone())
                }
            }),
            SocialTab::Search => social::user_rows(cache, self.social.search_results())
                .get(cursor)
                .map(|row| SocialSelection::User {
                    id: row.id.clone(),
                    friend: row.friend,
                }),
            SocialTab::Dms => social::dm_rows(cache)
                .get(cursor)
                .map(|row| SocialSelection::Dm(row.id.clone())),
        }
    }

    /// Открывает личный канал с выбранным другом или пользователем.
    pub fn social_open_dm(&mut self) -> bool {
        let request = match self.social_selection() {
            Some(SocialSelection::Friend(id) | SocialSelection::User { id, .. }) => {
                bs_api::CreateDmRequest::user(id)
            }
            _ => return false,
        };
        self.request(NetRequest::OpenDm(request))
    }

    /// Шлёт заявку в друзья выбранному пользователю из поиска.
    ///
    /// Для уже добавленного в друзья пользователя и для чужих вкладок
    /// возвращает `false`.
    pub fn social_send_request(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::User { id, friend: false }) => self.request(
                NetRequest::SendFriendRequest(bs_api::SendFriendRequest::username(id)),
            ),
            _ => false,
        }
    }

    /// Принимает выбранную входящую заявку.
    pub fn social_accept_request(&mut self) -> bool {
        self.respond_selected(bs_api::FriendRequestDecision::Accept)
    }

    /// Отклоняет выбранную входящую заявку.
    pub fn social_decline_request(&mut self) -> bool {
        self.respond_selected(bs_api::FriendRequestDecision::Decline)
    }

    fn respond_selected(&self, decision: bs_api::FriendRequestDecision) -> bool {
        match self.social_selection() {
            Some(SocialSelection::IncomingRequest(id)) => {
                self.request(NetRequest::RespondFriendRequest(id, decision))
            }
            _ => false,
        }
    }

    /// Отзывает выбранную исходящую заявку.
    pub fn social_cancel_request(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::OutgoingRequest(id)) => {
                self.request(NetRequest::CancelFriendRequest(id))
            }
            _ => false,
        }
    }

    /// Удаляет выбранного друга.
    pub fn social_remove_friend(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::Friend(id)) => self.request(NetRequest::RemoveFriend(id)),
            _ => false,
        }
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
