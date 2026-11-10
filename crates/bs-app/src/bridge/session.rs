//! Сессия одного инстанса: состояние и методы для экранов.

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

impl Session {
    /// Дескриптор runtime для фоновых задач вне моста (голос).
    pub(crate) fn runtime(&self) -> Handle {
        self.runtime.clone()
    }

    /// Создаёт мост над данным runtime.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            phase: Phase::Idle,
            events: None,
            commands: None,
            requests: None,
            responses: None,
            session: None,
            persist: None,
            friends: Vec::new(),
            friend_requests: Vec::new(),
            dm_channels: Vec::new(),
            search_results: Vec::new(),
            social_error: None,
            opened_dm: None,
            base: None,
            issued: Vec::new(),
            peering: Vec::new(),
            failure: None,
            registry_in: None,
            compose_in: Vec::new(),
            profile_in: Vec::new(),
            voice_in: Vec::new(),
            space_in: Vec::new(),
            group_in: Vec::new(),
            call_in: Vec::new(),
            new_messages: Vec::new(),
            raw_ready: Vec::new(),
            downloads_ready: Vec::new(),
            admin_in: Vec::new(),
            saved_in: Vec::new(),
            deleted_in: Vec::new(),
            adopted_in: None,
            synced: false,
            images_ready: Vec::new(),
            explore: None,
            directory: None,
            instance_info: None,
            instance_info_asked: false,
            explore_loading: false,
            opened_space: None,
            info: None,
            history: HashMap::new(),
            search: SearchState::default(),
            jump: None,
            detached: std::collections::HashSet::new(),
            cache: None,
            ready: false,
            session_lost: false,
            connection: ConnectionState::Connecting,
            last_error: None,
            notice: None,
        }
    }

    /// Текущая фаза сессии (нужна только тестам).
    #[cfg(test)]
    pub(crate) const fn phase(&self) -> Phase {
        self.phase
    }

    /// Забирает накопленные события из канала (без блокировки).
    pub(crate) fn poll(&mut self) {
        loop {
            let next = self.events.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(event)) => self.handle_event(event),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
        loop {
            let next = self.responses.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(response)) => self.handle_response(response),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
    }

    pub(super) fn handle_event(&mut self, event: BridgeEvent) {
        match event {
            BridgeEvent::Ready { payload } => {
                self.cache = Some(Cache::from_ready(*payload));
                self.history.clear();
                self.phase = Phase::Active;
                self.connection = ConnectionState::Connected;
                self.ready = true;
                self.session_lost = false;
                self.last_error = None;
                self.request(BridgeRequest::NotificationSettings);
            }
            BridgeEvent::Server { event } => self.apply_server_event(*event),
            BridgeEvent::State { state } => self.connection = state,
            BridgeEvent::Error { message } => self.last_error = Some(message),
            BridgeEvent::LoginFailed { message, kind } => {
                self.last_error = Some(message);
                self.failure = Some(kind);
            }
            BridgeEvent::Disconnected => {
                self.connection = ConnectionState::Stopped;
                self.last_error = Some(tr("соединение закрыто").to_owned());
            }
            BridgeEvent::Ended => self.finish_session(),
        }
    }

    /// Применяет событие сервера к кэшу.
    pub(super) fn apply_server_event(&mut self, event: ServerEvent) {
        match &event {
            // Отказ на действие пользователя: без этого сообщение, которое
            // сервер не принял, просто молча пропадало.
            ServerEvent::Error { message, code, .. } => {
                self.notice = Some(describe_server_error(code.as_deref(), message));
                return;
            }
            ServerEvent::DmCallIncoming {
                dm_channel_id,
                federated_call_id,
                caller_id,
                caller_name,
                livekit_url,
                livekit_token,
                ..
            } => {
                self.call_in.push(CallEvent::Incoming {
                    dm_channel_id: dm_channel_id.clone(),
                    federated_call_id: federated_call_id.clone(),
                    caller_id: caller_id.clone(),
                    caller_name: caller_name.clone(),
                    livekit_url: livekit_url.clone(),
                    livekit_token: livekit_token.clone(),
                });
                return;
            }
            ServerEvent::DmCallAccepted {
                dm_channel_id,
                federated_call_id,
                ..
            } => {
                self.call_in.push(CallEvent::Accepted {
                    dm_channel_id: dm_channel_id.clone(),
                    federated_call_id: federated_call_id.clone(),
                });
                return;
            }
            ServerEvent::DmCallRejected {
                dm_channel_id,
                federated_call_id,
            } => {
                self.call_in.push(CallEvent::Rejected {
                    dm_channel_id: dm_channel_id.clone(),
                    federated_call_id: federated_call_id.clone(),
                });
                return;
            }
            ServerEvent::DmCallEnded {
                dm_channel_id,
                federated_call_id,
            } => {
                self.call_in.push(CallEvent::Ended {
                    dm_channel_id: dm_channel_id.clone(),
                    federated_call_id: federated_call_id.clone(),
                });
                return;
            }
            ServerEvent::DmCallUndeliverable {
                dm_channel_id,
                federated_call_id,
                terminal,
                ..
            } => {
                self.call_in.push(CallEvent::Undeliverable {
                    dm_channel_id: dm_channel_id.clone(),
                    federated_call_id: federated_call_id.clone(),
                    terminal: *terminal,
                });
                return;
            }
            // `ready` после реконнекта пересобирает кэш без сообщений:
            // историю надо загрузить заново.
            ServerEvent::Ready(_) => self.history.clear(),
            ServerEvent::MessageCreated { message } => {
                self.new_messages.push(NewMessage {
                    dm: false,
                    channel_id: message.message.channel_id.clone(),
                    author_id: message.message.user_id.clone(),
                    content: message.message.content.clone().unwrap_or_default(),
                    mention_everyone: message.message.mention_everyone == Some(true),
                    mention_roles: message.message.mention_role_ids.clone().unwrap_or_default(),
                });
            }
            ServerEvent::DmMessageCreated { message } => {
                self.new_messages.push(NewMessage {
                    dm: true,
                    channel_id: message.message.dm_channel_id.clone(),
                    author_id: message.message.user_id.clone(),
                    content: message.message.content.clone().unwrap_or_default(),
                    mention_everyone: false,
                    mention_roles: Vec::new(),
                });
            }
            _ => {}
        }
        if let Some(cache) = self.cache.as_mut() {
            let _ = apply(cache, event);
        }
    }

    /// Фоновая задача завершилась: сессия окончательно закрыта.
    pub(super) fn finish_session(&mut self) {
        let was = self.phase;
        self.session = None;
        self.commands = None;
        self.requests = None;
        self.events = None;
        self.responses = None;
        self.cache = None;
        self.history.clear();
        self.friends.clear();
        self.friend_requests.clear();
        self.dm_channels.clear();
        self.search_results.clear();
        self.social_error = None;
        self.opened_dm = None;
        self.base = None;
        self.issued.clear();
        self.peering.clear();
        self.registry_in = None;
        self.saved_in.clear();
        self.deleted_in.clear();
        self.adopted_in = None;
        self.images_ready.clear();
        self.compose_in.clear();
        self.profile_in.clear();
        self.voice_in.clear();
        self.space_in.clear();
        self.group_in.clear();
        self.call_in.clear();
        self.admin_in.clear();
        self.explore = None;
        self.explore_loading = false;
        self.opened_space = None;
        self.info = None;
        self.notice = None;
        self.search = SearchState::default();
        self.jump = None;
        self.detached.clear();
        self.ready = false;
        self.connection = ConnectionState::Connecting;
        self.phase = Phase::Idle;
        // Отказ логина оставляет пользователя на форме; потеря живой сессии
        // и выход возвращают на неё.
        self.session_lost = matches!(was, Phase::Active | Phase::Closing);
        if was == Phase::Active && self.last_error.is_none() {
            self.last_error = Some(tr("соединение закрыто").to_owned());
        }
    }

    /// `true` ровно один раз — когда пришёл первый `ready`.
    pub(crate) fn take_ready(&mut self) -> bool {
        std::mem::take(&mut self.ready)
    }

    /// Идёт ли логин или выход: форму и кнопки надо заблокировать.
    pub(crate) const fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Connecting | Phase::Closing)
    }

    /// Сессия работает и получила первый снимок состояния.
    pub(crate) fn is_live(&self) -> bool {
        self.phase == Phase::Active && self.cache.is_some()
    }

    /// Сессия не запущена (нет ни входа, ни выхода в процессе).
    pub(crate) const fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    /// Показывает, что вход начат, ещё до запуска фоновой задачи
    /// (пока дом выдаёт секрет).
    pub(crate) fn mark_connecting(&mut self, base: &str) {
        self.phase = Phase::Connecting;
        self.base = Some(base.to_owned());
        self.last_error = None;
        self.connection = ConnectionState::Connecting;
    }

    /// Запоминает ошибку и возвращает сессию в покой.
    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
        self.phase = Phase::Idle;
        self.connection = ConnectionState::Connecting;
    }

    /// Показывает короткое сообщение в интерфейсе (однократно).
    pub(crate) fn set_info(&mut self, message: String) {
        self.info = Some(message);
    }

    /// Сколько каналов и бесед с непрочитанным.
    pub(crate) fn unread_total(&self) -> usize {
        self.cache.as_ref().map_or(0, |cache| {
            let channels = cache
                .all_channels()
                .filter(|c| cache.unread_count(&c.id) > 0)
                .count();
            let dms = cache
                .dm_channels()
                .filter(|d| cache.unread_count(&d.id) > 0)
                .count();
            channels + dms
        })
    }

    /// Последняя ошибка, если была.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Отказ сервера на последнее действие (показывается над лентой).
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Скрывает уведомление.
    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Кэш инстанса; `None` до первого `ready`.
    pub(crate) const fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Текущее состояние соединения (для строки статуса).
    pub(crate) const fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// `true` ровно один раз — если сессия закрыта и надо вернуться на логин.
    pub(crate) fn take_session_lost(&mut self) -> bool {
        std::mem::take(&mut self.session_lost)
    }

    /// Запускает логин в фоне.
    ///
    /// Работает только из фазы [`Phase::Idle`]: пока прежняя сессия не
    /// закрыта до конца, второй вход (в тот же или другой инстанс) отклоняется.
    pub(crate) fn start_login(
        &mut self,
        base: &str,
        username: &str,
        password: &str,
        transport: TransportPolicy,
    ) {
        if self.phase != Phase::Idle {
            self.last_error = Some(tr("Сначала выйдите из текущего инстанса").to_owned());
            return;
        }
        let Some(base) = normalize_base(base) else {
            self.last_error = Some(tr("заполните адрес, логин и пароль").to_owned());
            return;
        };
        if username.trim().is_empty() || password.is_empty() {
            self.last_error = Some(tr("заполните адрес, логин и пароль").to_owned());
            return;
        }
        // Проверяем адрес до сети: пользователь сразу видит причину отказа,
        // а пароль не уходит ни в какое соединение.
        match url::Url::parse(&base) {
            Ok(parsed) => {
                if let Err(err) = transport.check(&parsed) {
                    self.last_error = Some(err.to_string());
                    return;
                }
            }
            Err(err) => {
                self.last_error = Some(crate::tf!("некорректный адрес: {err}", err = err));
                return;
            }
        }

        self.launch(SessionParams {
            origin: Origin::Home,
            base,
            username: username.trim().to_owned(),
            password: Zeroizing::new(password.to_owned()),
            register: None,
            resume: None,
            persist: self.persist.clone(),
            transport,
        });
    }

    /// Задаёт файл, в котором хранится сессия домашнего инстанса.
    pub(crate) fn set_persist(&mut self, path: Option<std::path::PathBuf>) {
        self.persist = path;
    }

    /// Стирает сохранённую сессию (явный выход).
    pub(crate) fn forget_saved(&self) {
        if let Some(path) = self.persist.as_ref() {
            crate::saved_session::clear(path);
        }
    }

    /// Входит по сохранённому токену; `None`, если сохранённой сессии нет.
    pub(crate) fn resume_saved(&mut self) -> Option<(String, TransportPolicy)> {
        if self.phase != Phase::Idle {
            return None;
        }
        let path = self.persist.clone()?;
        let saved = crate::saved_session::load(&path)?;
        let base = normalize_base(&saved.base)?;
        let parsed = url::Url::parse(&base).ok()?;
        if saved.transport.check(&parsed).is_err() {
            return None;
        }
        self.launch(SessionParams {
            origin: Origin::Home,
            base: base.clone(),
            username: String::new(),
            password: Zeroizing::new(String::new()),
            register: None,
            resume: Some(saved.token),
            persist: Some(path),
            transport: saved.transport,
        });
        Some((base, saved.transport))
    }

    /// Запускает вход на другой инстанс по секрету, выданному домом.
    ///
    /// Если аккаунта там ещё нет, он создаётся (`register`); отказ
    /// регистрации не ошибка: аккаунт мог уже существовать.
    pub(crate) fn start_remote(
        &mut self,
        credential: &IssuedCredential,
        base: &str,
        display_name: Option<String>,
        transport: TransportPolicy,
        register: bool,
    ) {
        // Сессия могла быть помечена «подключается» (ждали секрет), но
        // фоновая задача ещё не запущена: только в этом случае входим.
        if self.session.is_some() {
            return;
        }
        let Ok(url) = url::Url::parse(base) else {
            self.fail(tr("некорректный адрес инстанса"));
            return;
        };
        if let Err(err) = transport.check(&url) {
            self.fail(err.to_string());
            return;
        }
        self.launch(SessionParams {
            origin: Origin::Remote(url),
            base: base.to_owned(),
            username: credential.username.clone(),
            password: Zeroizing::new(credential.secret.expose().to_owned()),
            register: register.then(|| RegisterRequest {
                username: credential.username.clone(),
                password: credential.secret.expose().to_owned(),
                display_name,
                home_instance: Some(credential.home_host.clone()),
                home_user_id: Some(credential.home_user_id.clone()),
                ..RegisterRequest::default()
            }),
            resume: None,
            persist: None,
            transport,
        });
    }

    /// Регистрирует новый аккаунт на домашнем инстансе и сразу входит в него.
    pub(crate) fn start_register(
        &mut self,
        base: &str,
        request: RegisterRequest,
        transport: TransportPolicy,
    ) {
        if self.phase != Phase::Idle {
            self.last_error = Some(tr("Сначала выйдите из текущего инстанса").to_owned());
            return;
        }
        let Some(base) = normalize_base(base) else {
            self.last_error = Some(tr("укажите адрес инстанса").to_owned());
            return;
        };
        match url::Url::parse(&base) {
            Ok(parsed) => {
                if let Err(err) = transport.check(&parsed) {
                    self.last_error = Some(err.to_string());
                    return;
                }
            }
            Err(err) => {
                self.last_error = Some(crate::tf!("некорректный адрес: {err}", err = err));
                return;
            }
        }
        self.launch(SessionParams {
            origin: Origin::Home,
            base,
            username: request.username.clone(),
            password: Zeroizing::new(request.password.clone()),
            register: Some(request),
            resume: None,
            persist: self.persist.clone(),
            transport,
        });
    }

    /// Входит на чужой инстанс **собственным** паролем аккаунта там (без
    /// регистрации): запасной путь, когда инстанс не принял выданный секрет.
    pub(crate) fn start_remote_login(
        &mut self,
        base: &str,
        username: &str,
        password: &Secret,
        transport: TransportPolicy,
    ) {
        if self.session.is_some() {
            return;
        }
        let Ok(url) = url::Url::parse(base) else {
            self.fail(tr("некорректный адрес инстанса"));
            return;
        };
        if let Err(err) = transport.check(&url) {
            self.fail(err.to_string());
            return;
        }
        self.launch(SessionParams {
            origin: Origin::Remote(url),
            base: base.to_owned(),
            username: username.to_owned(),
            password: Zeroizing::new(password.expose().to_owned()),
            register: None,
            resume: None,
            persist: None,
            transport,
        });
    }

    /// Общий запуск фоновой сессии.
    pub(super) fn launch(&mut self, params: SessionParams) {
        self.phase = Phase::Connecting;
        self.base = Some(params.base.clone());
        self.last_error = None;
        self.failure = None;
        self.session_lost = false;
        self.connection = ConnectionState::Connecting;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, responses_rx) = std_mpsc::channel();
        self.events = Some(events_rx);
        self.commands = Some(commands_tx);
        self.requests = Some(requests_tx);
        self.responses = Some(responses_rx);

        self.session = Some(self.runtime.spawn(run_session(
            params,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        )));
    }

    /// Адрес инстанса этой сессии.
    pub(crate) fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// Забирает выданные секреты (у домашней сессии).
    pub(crate) fn take_issued(&mut self) -> Vec<(String, Result<IssuedCredential, String>)> {
        std::mem::take(&mut self.issued)
    }

    /// Причина последнего неудачного входа.
    pub(crate) const fn failure(&self) -> Option<FailureKind> {
        self.failure
    }

    /// Забирает реестр подключений (у домашней сессии).
    pub(crate) fn take_registry(&mut self) -> Option<Result<RegistryResponse, String>> {
        self.registry_in.take()
    }

    /// Забирает итоги записи реестра (у домашней сессии).
    pub(crate) fn take_saved(&mut self) -> Vec<Result<(), String>> {
        std::mem::take(&mut self.saved_in)
    }

    /// Забирает итоги удаления учёток (у домашней сессии).
    pub(crate) fn take_deleted(
        &mut self,
    ) -> Vec<Result<HashMap<String, IdentityDeleteResult>, String>> {
        std::mem::take(&mut self.deleted_in)
    }

    /// Забирает итог смены пароля на выданный секрет.
    pub(crate) fn take_adopted(&mut self) -> Option<Result<(), String>> {
        self.adopted_in.take()
    }

    /// Забирает ответы о пиринге (у домашней сессии).
    pub(crate) fn take_peering(&mut self) -> Vec<(String, Result<String, String>)> {
        std::mem::take(&mut self.peering)
    }

    /// Выходит из инстанса: прерывает фоновую сессию.
    ///
    /// Мост остаётся занятым ([`Phase::Closing`]) до события `Ended`, так что
    /// новый вход невозможен, пока старое соединение не закрыто полностью.
    pub(crate) fn logout(&mut self) {
        if !matches!(self.phase, Phase::Connecting | Phase::Active) {
            return;
        }
        self.phase = Phase::Closing;
        self.commands = None;
        self.requests = None;
        if let Some(session) = self.session.as_ref() {
            session.abort();
        }
    }

    /// Отправляет сообщение в канал или личный канал (в фоне).
    ///
    /// Для ЛС сервер ждёт отдельное событие `dm_message_create`: обычный
    /// `message_create` с id личного канала он молча отбрасывает.
    pub(crate) fn send_message(
        &self,
        kind: ChannelKind,
        channel_id: String,
        content: String,
        reply_to_id: Option<String>,
    ) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageCreate {
                channel_id,
                content,
                reply_to_id,
            },
            ChannelKind::Dm => ClientEvent::DmMessageCreate {
                dm_channel_id: channel_id,
                content: Some(content),
                attachments: None,
                reply_to_id,
            },
        });
    }

    /// Отправляет команду серверу, если сессия жива.
    pub(crate) fn send(&self, event: ClientEvent) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(event);
        }
    }

    /// Правит своё сообщение.
    pub(crate) fn edit_message(&self, kind: ChannelKind, message_id: String, content: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageEdit {
                message_id,
                content,
            },
            ChannelKind::Dm => ClientEvent::DmMessageEdit {
                message_id,
                content,
            },
        });
    }

    /// Удаляет своё сообщение.
    pub(crate) fn delete_message(&self, kind: ChannelKind, message_id: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageDelete { message_id },
            ChannelKind::Dm => ClientEvent::DmMessageDelete { message_id },
        });
    }

    /// Ставит реакцию.
    pub(crate) fn add_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionAdd { message_id, emoji });
    }

    /// Убирает реакцию.
    pub(crate) fn remove_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionRemove { message_id, emoji });
    }

    /// Подтверждает прочтение канала до сообщения.
    pub(crate) fn ack_channel(&self, channel_id: String, message_id: String) {
        self.send(ClientEvent::ChannelAck {
            channel_id,
            message_id,
        });
    }

    /// Отправляет REST-запрос; `false`, если сессии нет.
    pub(crate) fn request(&self, request: BridgeRequest) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|requests| requests.send(request).is_ok())
    }

    /// Забирает накопленные ответы настроек.
    pub(crate) fn take_admin_events(&mut self) -> Vec<AdminEvent> {
        std::mem::take(&mut self.admin_in)
    }

    /// Забирает скачанные файлы.
    /// Забирает ход скачиваний на диск.
    pub(crate) fn take_downloads(&mut self) -> Vec<(u64, DownloadState)> {
        std::mem::take(&mut self.downloads_ready)
    }

    pub(crate) fn take_raw(&mut self) -> Vec<(u64, Result<Vec<u8>, String>)> {
        std::mem::take(&mut self.raw_ready)
    }

    /// Забирает накопленные события DM-звонков.
    pub(crate) fn take_call_events(&mut self) -> Vec<CallEvent> {
        std::mem::take(&mut self.call_in)
    }

    /// Запоминает итог действия над пространством; выход убирает его из кэша.
    pub(super) fn apply_space_event(&mut self, event: SpaceEvent) {
        match &event {
            SpaceEvent::Left {
                space_id,
                result: Ok(()),
            }
            | SpaceEvent::Admin {
                space_id,
                op: SpaceOp::Delete,
                result: Ok(()),
                ..
            } => {
                if let Some(cache) = self.cache.as_mut() {
                    cache.remove_space(space_id);
                }
                self.explore = None;
            }
            SpaceEvent::Notified {
                result: Ok(setting),
                ..
            } => {
                if let Some(cache) = self.cache.as_mut() {
                    cache.apply_notification_setting(setting.clone());
                }
            }
            SpaceEvent::Admin {
                space: Some(space), ..
            } => {
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_space((**space).clone());
                }
            }
            _ => {}
        }
        self.space_in.push(event);
    }

    /// Забирает итоги действий над группами.
    pub(crate) fn take_group_events(&mut self) -> Vec<GroupEvent> {
        std::mem::take(&mut self.group_in)
    }

    /// Забирает итоги действий над пространствами.
    pub(crate) fn take_space_events(&mut self) -> Vec<SpaceEvent> {
        std::mem::take(&mut self.space_in)
    }

    /// Забирает сообщения, пришедшие с прошлого вызова.
    pub(crate) fn take_new_messages(&mut self) -> Vec<NewMessage> {
        std::mem::take(&mut self.new_messages)
    }

    /// Забирает накопленные ответы на запросы токена голоса.
    pub(crate) fn take_voice_tokens(&mut self) -> Vec<VoiceTokenEvent> {
        std::mem::take(&mut self.voice_in)
    }

    /// Забирает накопленные ответы профилей.
    pub(crate) fn take_profile_events(&mut self) -> Vec<ProfileEvent> {
        std::mem::take(&mut self.profile_in)
    }

    /// Забирает накопленные события композера.
    pub(crate) fn take_compose_events(&mut self) -> Vec<ComposeEvent> {
        std::mem::take(&mut self.compose_in)
    }

    /// Отправляет сообщение в канал пространства (REST, нужен для вложений).
    pub(crate) fn send_rest(&self, channel_id: String, body: bs_api::SendMessageBody) {
        self.request(BridgeRequest::SendRest { channel_id, body });
    }

    /// Отправляет личное сообщение с вложениями и ответом.
    pub(crate) fn send_dm_with(
        &self,
        channel_id: String,
        content: Option<String>,
        attachments: Vec<String>,
        reply_to_id: Option<String>,
    ) {
        self.send(ClientEvent::DmMessageCreate {
            dm_channel_id: channel_id,
            content,
            attachments: if attachments.is_empty() {
                None
            } else {
                Some(attachments)
            },
            reply_to_id,
        });
    }

    /// Последние полученные друзья.
    pub(crate) fn friends(&self) -> &[bs_types::Friend] {
        &self.friends
    }

    /// Последние полученные заявки.
    pub(crate) fn friend_requests(&self) -> &[bs_types::FriendRequest] {
        &self.friend_requests
    }

    /// Последние результаты поиска пользователей.
    pub(crate) fn search_results(&self) -> &[bs_types::User] {
        &self.search_results
    }

    /// `true` ровно один раз — если только что открыли личный канал.
    pub(crate) fn take_opened_dm(&mut self) -> Option<String> {
        self.opened_dm.take()
    }

    /// Адрес инстанса и канал запросов живой сессии — для кэша картинок.
    pub(crate) fn session_link(
        &self,
    ) -> Option<(String, tokio_mpsc::UnboundedSender<BridgeRequest>)> {
        Some((self.base.clone()?, self.requests.clone()?))
    }

    /// Забирает картинки, пришедшие с прошлого кадра.
    pub(crate) fn take_images(
        &mut self,
    ) -> Vec<(bs_api::AssetPath, Result<bs_media::Decoded, String>)> {
        std::mem::take(&mut self.images_ready)
    }

    /// Последняя страница каталога пространств.
    pub(crate) fn explore(&self) -> Option<&bs_api::ExplorePage> {
        self.explore.as_ref()
    }

    /// Идёт ли запрос каталога.
    pub(crate) const fn explore_loading(&self) -> bool {
        self.explore_loading
    }

    /// Возвращает пространство, которое только что создали или в которое вступили.
    pub(crate) fn take_opened_space(&mut self) -> Option<String> {
        self.opened_space.take()
    }

    /// Короткое сообщение об успехе; читается один раз.
    pub(crate) fn take_info(&mut self) -> Option<String> {
        self.info.take()
    }

    /// Запрашивает каталог (свой и общий) и помечает загрузку.
    pub(crate) fn load_explore(&mut self, query: &str) {
        if self.request(BridgeRequest::Explore(query.to_owned())) {
            self.explore_loading = true;
        }
        self.request(BridgeRequest::Directory(query.to_owned()));
    }

    /// Общий каталог других инстансов, если пришёл.
    pub(crate) fn directory(&self) -> Option<&Result<Vec<bs_api::DirectoryEntry>, String>> {
        self.directory.as_ref()
    }

    /// Запрашивает сведения об инстансе один раз за сессию.
    pub(crate) fn ensure_instance_info(&mut self) {
        if self.instance_info.is_none() && !self.instance_info_asked && self.is_live() {
            self.instance_info_asked = self.request(BridgeRequest::InstanceInfo);
        }
    }

    /// Сведения об инстансе, если уже пришли.
    pub(crate) fn instance_info(&self) -> Option<&Result<Box<bs_api::InstanceInfo>, String>> {
        self.instance_info.as_ref()
    }

    /// Ошибка последнего REST-запроса.
    pub(crate) fn social_error(&self) -> Option<&str> {
        self.social_error.as_deref()
    }

    /// Состояние загрузки истории канала.
    pub(crate) fn history_state(&self, channel_id: &str) -> Option<&HistoryState> {
        self.history.get(channel_id)
    }

    /// Просит самые новые сообщения канала, если ещё не просили.
    ///
    /// Безопасно вызывать каждый кадр: повторный запрос не уходит, пока
    /// состояние канала известно.
    pub(crate) fn ensure_history(&mut self, kind: ChannelKind, channel_id: &str) {
        if self.history.contains_key(channel_id) {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: None,
        };
        if self.request(request) {
            self.history.insert(
                channel_id.to_owned(),
                HistoryState {
                    loading: true,
                    ..HistoryState::default()
                },
            );
        }
    }

    /// Состояние поиска по сообщениям.
    pub(crate) const fn search_state(&self) -> &SearchState {
        &self.search
    }

    /// Сбрасывает результаты поиска (панель закрыта или сменился канал).
    pub(crate) fn clear_search(&mut self) {
        self.search.seq += 1;
        self.search.loading = false;
        self.search.hits.clear();
        self.search.total = 0;
        self.search.error = None;
    }

    /// Запускает поиск; `more` — следующая страница тех же результатов.
    pub(crate) fn run_search(
        &mut self,
        kind: ChannelKind,
        channel_id: &str,
        mut query: bs_api::MessageSearchQuery,
        more: bool,
    ) {
        if more {
            if self.search.loading {
                return;
            }
            query.offset = u32::try_from(self.search.hits.len()).ok();
        } else {
            self.clear_search();
        }
        query.limit = Some(SEARCH_PAGE);
        let request = BridgeRequest::Search {
            channel_id: channel_id.to_owned(),
            kind,
            query,
            seq: self.search.seq,
        };
        self.search.error = None;
        self.search.loading = self.request(request);
    }

    /// Переходит к сообщению: если оно уже загружено — сразу, иначе
    /// грузит окно вокруг него и заменяет им ленту канала.
    pub(crate) fn jump_to(&mut self, kind: ChannelKind, channel_id: &str, message_id: &str) {
        let known = self.cache.as_ref().is_some_and(|cache| match kind {
            ChannelKind::Space => cache
                .messages(channel_id)
                .iter()
                .any(|m| m.message.id == message_id),
            ChannelKind::Dm => cache
                .dm_messages(channel_id)
                .iter()
                .any(|m| m.message.id == message_id),
        });
        if known {
            self.jump = Some((channel_id.to_owned(), message_id.to_owned()));
            return;
        }
        let _ = self.request(BridgeRequest::Around {
            channel_id: channel_id.to_owned(),
            kind,
            message_id: message_id.to_owned(),
        });
    }

    /// Сообщение, к которому надо прокрутить ленту этого канала (однократно).
    pub(crate) fn take_jump(&mut self, channel_id: &str) -> Option<String> {
        if self
            .jump
            .as_ref()
            .is_some_and(|(chan, _)| chan == channel_id)
        {
            return self.jump.take().map(|(_, id)| id);
        }
        None
    }

    /// Показано ли окно вокруг найденного сообщения вместо свежих.
    pub(crate) fn is_detached(&self, channel_id: &str) -> bool {
        self.detached.contains(channel_id)
    }

    /// Возвращает канал к свежим сообщениям: старое окно забывается, лента
    /// грузится заново.
    pub(crate) fn return_to_present(&mut self, channel_id: &str) {
        self.detached.remove(channel_id);
        self.history.remove(channel_id);
        if let Some(cache) = self.cache.as_mut() {
            cache.forget_messages(channel_id);
        }
    }

    /// Вливает окно вокруг сообщения: лента канала заменяется им.
    pub(super) fn apply_around(
        &mut self,
        channel_id: &str,
        message_id: String,
        items: HistoryItems,
    ) {
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        match items {
            HistoryItems::Space(page) => cache.replace_history(channel_id, page),
            HistoryItems::Dm(page) => cache.replace_dm_history(channel_id, page),
        }
        self.history
            .insert(channel_id.to_owned(), HistoryState::default());
        self.detached.insert(channel_id.to_owned());
        self.jump = Some((channel_id.to_owned(), message_id));
    }

    /// Подгружает страницу старее уже загруженного (или повторяет первую
    /// страницу после ошибки).
    pub(crate) fn load_older(&mut self, kind: ChannelKind, channel_id: &str) {
        let Some(state) = self.history.get(channel_id) else {
            return;
        };
        if state.loading || state.exhausted {
            return;
        }
        let oldest = self.cache.as_ref().and_then(|cache| match kind {
            ChannelKind::Space => cache
                .messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
            ChannelKind::Dm => cache
                .dm_messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
        });
        if oldest.is_none() && state.error.is_none() {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: oldest,
        };
        if self.request(request)
            && let Some(state) = self.history.get_mut(channel_id)
        {
            state.loading = true;
            state.error = None;
        }
    }
}
