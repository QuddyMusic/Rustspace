//! Состояние TUI: экраны, форма логина, кэши инстансов и подключение.

use std::collections::HashMap;

use bs_core::{Cache, MultiCache};
use bs_instances::ConnectionState;
use bs_types::{ClientEvent, Origin, ServerEvent};

use crate::chat::ChatState;
use crate::login::LoginForm;
use crate::net::{NetEvent, NetPayload, NetRequest, NetResponse, NetWorker};
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

/// Статус для инстанса, о котором ничего не известно.
static IDLE: Status = Status::Idle;

/// Корневое состояние приложения.
///
/// Кэшей несколько: по одному на каждый подключённый инстанс. Активный
/// инстанс задаёт, что видно на экране, но события приходят от всех сразу.
pub struct App {
    screen: Screen,
    running: bool,
    form: LoginForm,
    worker: Option<NetWorker>,
    multi: MultiCache,
    active: Origin,
    home_base: Option<String>,
    states: HashMap<Origin, Status>,
    chat: ChatState,
    social: SocialState,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("running", &self.running)
            .field("form", &self.form)
            .field("worker", &self.worker.is_some())
            .field("instances", &self.multi.len())
            .field("active", &self.active)
            .field("home_base", &self.home_base)
            .field("states", &self.states)
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
            worker: None,
            multi: MultiCache::new(),
            active: Origin::Home,
            home_base: None,
            states: HashMap::new(),
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

    /// Инстанс, на который смотрит интерфейс.
    pub fn active(&self) -> &Origin {
        &self.active
    }

    /// Все известные инстансы: домашний первым, затем удалённые.
    pub fn instances(&self) -> Vec<Origin> {
        self.multi.origins()
    }

    /// Статус конкретного инстанса.
    pub fn status_of(&self, origin: &Origin) -> &Status {
        self.states.get(origin).unwrap_or(&IDLE)
    }

    /// Состояние подключения активного инстанса.
    pub fn status(&self) -> &Status {
        self.status_of(&self.active)
    }

    /// Кэш активного инстанса; `None` до первого `ready`.
    pub fn cache(&self) -> Option<&Cache> {
        self.multi.cache(&self.active)
    }

    /// Все кэши (для сводок и отладки).
    pub fn multi(&self) -> &MultiCache {
        &self.multi
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
        self.form.is_submittable()
    }

    /// Выводит origin для адреса: первый адрес становится домашним.
    fn origin_for(&mut self, base: &str) -> Origin {
        match self.home_base.clone() {
            None => {
                self.home_base = Some(base.to_owned());
                Origin::Home
            }
            Some(home) if home == base => Origin::Home,
            Some(_) => Origin::parse_web_str(base).unwrap_or(Origin::Home),
        }
    }

    /// Запускает логин в фоне. Возвращает `false`, если это неуместно.
    pub fn submit_login(&mut self) -> bool {
        if !self.can_submit() {
            return false;
        }
        let Some(creds) = self.form.credentials() else {
            return false;
        };
        let origin = self.origin_for(&creds.base);
        let worker = self.worker.get_or_insert_with(NetWorker::spawn);
        if !worker.connect(origin.clone(), creds) {
            return false;
        }
        self.states.insert(origin.clone(), Status::Connecting);
        self.active = origin;
        self.screen = Screen::Chat;
        true
    }

    /// Открывает форму для добавления ещё одного инстанса.
    pub fn begin_add_instance(&mut self) {
        self.form = LoginForm::new();
        self.screen = Screen::Login;
    }

    /// Отменяет добавление инстанса: возвращает туда, откуда пришли.
    ///
    /// Возвращает `false`, если подключённых инстансов ещё нет и отменять
    /// некуда (тогда Esc должен выходить из приложения).
    pub fn cancel_login(&mut self) -> bool {
        if self.multi.is_empty() {
            return false;
        }
        self.screen = Screen::Chat;
        self.form = LoginForm::new();
        true
    }

    /// Подключён ли хотя бы один инстанс.
    pub fn has_instances(&self) -> bool {
        !self.multi.is_empty()
    }

    /// Переключает активный инстанс на следующий.
    pub fn next_instance(&mut self) {
        self.cycle_instance(1);
    }

    /// Переключает активный инстанс на предыдущий.
    pub fn prev_instance(&mut self) {
        self.cycle_instance(-1);
    }

    fn cycle_instance(&mut self, delta: isize) {
        let origins = self.multi.origins();
        if origins.len() < 2 {
            return;
        }
        let current = origins.iter().position(|o| *o == self.active);
        let next = current.map_or(0, |index| {
            let len = origins.len() as isize;
            ((index as isize + delta).rem_euclid(len)) as usize
        });
        self.active = origins[next].clone();
        self.after_switch();
    }

    /// Переключает на конкретный инстанс, если он известен.
    pub fn switch_to(&mut self, origin: Origin) -> bool {
        if !self.multi.contains(&origin) {
            return false;
        }
        self.active = origin;
        self.after_switch();
        true
    }

    /// Общие действия после смены инстанса: пересчёт курсоров и подгрузка.
    fn after_switch(&mut self) {
        let active = self.active.clone();
        self.chat.follow(&active);
        self.chat.clamp_multi(&self.multi);
        if let Some(cache) = self.multi.cache(&self.active) {
            self.social.clamp(cache);
        }
        self.screen = Screen::Chat;
        self.refresh_social();
    }

    /// Отключается от активного инстанса и убирает его из списка.
    pub fn disconnect_active(&mut self) {
        if !self.multi.contains(&self.active) && self.multi.is_empty() {
            return;
        }
        let origin = self.active.clone();
        self.multi.remove(&origin);
        self.states.remove(&origin);
        if let Some(worker) = self.worker.as_ref() {
            worker.disconnect(origin);
        }
        if let Some(next) = self.multi.origins().first().cloned() {
            self.active = next;
            self.after_switch();
        } else {
            self.active = Origin::Home;
            self.screen = Screen::Login;
            self.chat = ChatState::new();
            self.social = SocialState::new();
        }
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
        if response.origin != self.active {
            // Ответ от фонового инстанса не должен подменять экран.
            return;
        }
        match response.payload {
            NetPayload::Friends(_) | NetPayload::FriendRequests(_) | NetPayload::DmChannels(_) => {
                if let Some(cache) = self.multi.cache(&self.active) {
                    self.social.clamp(cache);
                }
            }
            NetPayload::Users(users) => {
                self.social.set_search_results(users);
                self.social.set_message(None);
            }
            NetPayload::DmChannel(channel) => {
                self.social
                    .set_message(Some(format!("личный канал: {}", channel.id)));
            }
            NetPayload::Done => {
                self.social.set_message(None);
                self.refresh_social();
            }
            NetPayload::Failed(message) => {
                self.social.set_message(Some(message));
            }
        }
    }

    /// Применяет одно событие фоновой сессии.
    pub fn apply_net_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Ready { origin, event } => {
                if let ServerEvent::Ready(payload) = *event {
                    self.multi.insert_ready(origin.clone(), *payload);
                }
                self.states.insert(origin.clone(), Status::Connected);
                self.active = origin.clone();
                self.screen = Screen::Chat;
                self.chat.follow(&origin);
                self.chat.clamp_multi(&self.multi);
                if let Some(cache) = self.multi.cache(&self.active) {
                    self.social.clamp(cache);
                }
                self.refresh_social();
            }
            NetEvent::Server { origin, event } => {
                if let ServerEvent::Ready(payload) = *event {
                    // Повторный `ready` (переподключение): пересобираем кэш.
                    self.multi.insert_ready(origin.clone(), *payload);
                    self.states.insert(origin.clone(), Status::Connected);
                } else if self.multi.apply(&origin, *event).is_accepted()
                    && origin == self.active
                    && let Some(cache) = self.multi.cache(&self.active)
                {
                    self.chat.clamp_multi(&self.multi);
                    self.social.clamp(cache);
                }
            }
            NetEvent::State { origin, state } => {
                let status = match state {
                    ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                        Status::Connecting
                    }
                    ConnectionState::Connected => Status::Connected,
                    ConnectionState::Stopped => Status::Failed("соединение остановлено".to_owned()),
                };
                self.states.insert(origin, status);
            }
            NetEvent::Disconnected { origin } => {
                self.multi.remove(&origin);
                self.states.remove(&origin);
                if origin == self.active {
                    if let Some(next) = self.multi.origins().first().cloned() {
                        self.active = next;
                        self.after_switch();
                    } else {
                        self.active = Origin::Home;
                        self.screen = Screen::Login;
                    }
                }
            }
            NetEvent::Failed { origin, message } => {
                self.states.insert(origin, Status::Failed(message));
            }
        }
    }

    /// Отправляет набранный текст в текущий канал.
    ///
    /// В режиме правки вместо нового сообщения уходит `message_edit`.
    /// Возвращает `false`, если отправлять некуда или нечего.
    pub fn send_composer(&mut self) -> bool {
        if self.cache().is_none() {
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
        let cache = self.cache()?;
        self.chat.current_channel_id(cache)
    }

    /// Отправляет событие в фоновую сессию активного инстанса.
    fn send_event(&self, event: ClientEvent) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(&self.active, event))
    }

    /// Сдвигает курсор по ленте сообщений текущего канала.
    pub fn nav_message(&mut self, delta: isize) {
        let active = self.active.clone();
        let Some(cache) = self.multi.cache(&active) else {
            return;
        };
        self.chat.move_selection(cache, delta);
    }

    /// Начинает правку выбранного своего сообщения.
    ///
    /// Возвращает `false`, если сообщение не выбрано или написано не нами.
    pub fn edit_selected(&mut self) -> bool {
        let Some(cache) = self.cache() else {
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
        let Some(cache) = self.cache() else {
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
        let cache = self.cache()?;
        self.chat
            .selected_message(cache)
            .map(|message| message.message.id.clone())
    }

    /// Подтверждает прочтение текущего канала до последнего сообщения.
    ///
    /// Повторный вызов для той же пары «канал, сообщение» ничего не шлёт.
    pub fn ack_current_channel(&mut self) -> bool {
        let Some(cache) = self.cache() else {
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
        let Some(cache) = self.cache() else {
            return;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return;
        };
        if !self.chat.should_send_typing(&channel_id) {
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            worker.send(&self.active, ClientEvent::TypingStart { channel_id });
        }
    }

    /// Сдвигает выбор канала на `delta`.
    pub fn nav_channel(&mut self, delta: isize) {
        self.chat.move_channel_multi(&self.multi, delta);
        self.sync_active();
        self.ack_current_channel();
    }

    /// Сдвигает выбор пространства на `delta`.
    pub fn nav_space(&mut self, delta: isize) {
        self.chat.move_space_multi(&self.multi, delta);
        self.sync_active();
        self.ack_current_channel();
    }

    /// Держит `active` в согласии с инстансом, который выбран в сайдбаре.
    ///
    /// Объединённый сайдбар позволяет уйти на пространство другого инстанса
    /// стрелками — тогда меняется и `active`, и все запросы/события уходят
    /// уже туда.
    fn sync_active(&mut self) {
        let origin = self.chat.origin().clone();
        if origin != self.active && self.multi.contains(&origin) {
            self.active = origin;
            if let Some(cache) = self.multi.cache(&self.active) {
                self.social.clamp(cache);
            }
            self.refresh_social();
        }
    }

    /// Отправляет REST-запрос фоновой сессии.
    fn request(&self, request: NetRequest) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.request(&self.active, request))
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
        {
            let active = self.active.clone();
            let Some(cache) = self.multi.cache(&active) else {
                return;
            };
            self.social.move_cursor(cache, delta);
        }
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
        let cache = self.cache()?;
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

    /// Отключается от всех инстансов и возвращает на экран логина.
    pub fn logout(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join();
        }
        self.multi = MultiCache::new();
        self.states.clear();
        self.active = Origin::Home;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_login_becomes_home() {
        let mut app = App::new();
        assert_eq!(app.origin_for("http://localhost:3000"), Origin::Home);
        assert_eq!(app.origin_for("http://localhost:3000"), Origin::Home);
    }

    #[test]
    fn second_login_becomes_remote() {
        let mut app = App::new();
        app.origin_for("http://localhost:3000");
        let origin = app.origin_for("https://chat.example.com");
        assert!(matches!(origin, Origin::Remote(_)));
        assert_eq!(origin.as_web_str(), "https://chat.example.com");
    }

    #[test]
    fn unknown_instance_status_is_idle() {
        let app = App::new();
        assert_eq!(app.status(), &Status::Idle);
        assert_eq!(app.status_of(&Origin::Home), &Status::Idle);
    }

    #[test]
    fn switching_to_unknown_instance_fails() {
        let mut app = App::new();
        let remote = Origin::parse_web_str("https://chat.example.com").unwrap();
        assert!(!app.switch_to(remote));
    }

    #[test]
    fn cycling_with_single_instance_is_noop() {
        let mut app = App::new();
        app.next_instance();
        assert_eq!(app.active(), &Origin::Home);
        app.prev_instance();
        assert_eq!(app.active(), &Origin::Home);
    }
}
