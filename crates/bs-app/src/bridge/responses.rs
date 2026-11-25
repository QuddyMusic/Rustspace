//! Применение ответов фоновой задачи к состоянию сессии.

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

impl Session {
    /// Вливает страницу истории в кэш и обновляет состояние канала.
    pub(super) fn apply_history(&mut self, channel_id: &str, items: HistoryItems) {
        let Some(state) = self.history.get_mut(channel_id) else {
            // Пришла после `ready`, который уже сбросил кэш и состояние.
            return;
        };
        state.loading = false;
        state.error = None;
        state.exhausted = items.len() < usize::from(HISTORY_PAGE);
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        match items {
            HistoryItems::Space(page) => {
                cache.merge_history(channel_id, page);
            }
            HistoryItems::Dm(mut page) => {
                let direct = cache.dm_channel(channel_id).is_some_and(e2ee::is_direct);
                if let Some(e2ee) = self.e2ee.get_mut().as_mut() {
                    e2ee.incoming_page(&mut page, direct);
                }
                cache.merge_dm_history(channel_id, page);
            }
        }
    }

    /// Применяет ответ REST-запроса.
    pub(super) fn handle_response(&mut self, response: BridgeResponse) {
        match response {
            BridgeResponse::History { channel_id, items } => {
                self.apply_history(&channel_id, items);
            }
            BridgeResponse::SearchDone { seq, result } => self.apply_search(seq, result),
            BridgeResponse::AroundDone {
                channel_id,
                message_id,
                result,
            } => match result {
                Ok(items) => self.apply_around(&channel_id, message_id, items),
                Err(message) => {
                    self.info = Some(crate::tf!(
                        "Не удалось перейти к сообщению: {message}",
                        message = message
                    ));
                }
            },
            BridgeResponse::HistoryFailed {
                channel_id,
                message,
            } => {
                if let Some(state) = self.history.get_mut(&channel_id) {
                    state.loading = false;
                    state.error = Some(message);
                }
            }
            BridgeResponse::Quiet => {}
            BridgeResponse::E2eeBackup(result) => self.e2ee_backup = Some(result),
            BridgeResponse::Gallery(page) => self.gallery_pages.push(page),
            BridgeResponse::StreamPreview {
                channel_id,
                user_id,
                jpeg,
            } => {
                if let Some(entry) = self.stream_previews.get_mut(&(channel_id, user_id)) {
                    entry.0 = jpeg.map(std::sync::Arc::from);
                    entry.2 += 1;
                }
            }
            BridgeResponse::SpaceProfiles { space_id, result } => {
                if let Ok(list) = result {
                    let at = self
                        .space_profiles
                        .get(&space_id)
                        .map_or_else(web_time::Instant::now, |(_, at)| *at);
                    self.space_profiles.insert(space_id, (list, at));
                }
            }
            BridgeResponse::SpaceProfileSaved { space_id, result } => {
                self.space_profile_saved = Some(result.as_ref().map(drop).map_err(Clone::clone));
                if let Ok(list) = result {
                    self.space_profiles
                        .insert(space_id, (list, web_time::Instant::now()));
                }
            }
            BridgeResponse::E2eePeerKey {
                dm_channel_id,
                public_key,
            } => {
                if let Some(e2ee) = self.e2ee.get_mut().as_mut() {
                    e2ee.directory_key(&dm_channel_id, &public_key);
                }
                // Шифрование могло включиться само: свой ключ — собеседнику.
                self.flush_e2ee();
            }
            BridgeResponse::NotificationSettings(list) => {
                if let Some(cache) = self.cache.as_mut() {
                    cache.set_notification_settings(list);
                }
            }
            BridgeResponse::Friends(_)
            | BridgeResponse::FriendRequests(_)
            | BridgeResponse::Users(_)
            | BridgeResponse::DmChannels(_)
            | BridgeResponse::DmChannel(_)
            | BridgeResponse::Explore(_)
            | BridgeResponse::Space(_)
            | BridgeResponse::Profile(_)
            | BridgeResponse::JoinRequested
            | BridgeResponse::Done
            | BridgeResponse::Failed(_) => self.apply_social(response),
            BridgeResponse::GroupAction(event, channel) => self.apply_group_action(event, channel),
            other => self.enqueue(other),
        }
    }

    fn apply_search(&mut self, seq: u64, result: Result<(Vec<SearchHit>, i64), String>) {
        if seq != self.search.seq {
            return;
        }
        self.search.loading = false;
        match result {
            Ok((hits, total)) => {
                self.search.hits.extend(hits);
                self.search.total = total;
            }
            Err(message) => self.search.error = Some(message),
        }
    }

    /// Друзья, личные каналы, каталог, профиль — всё, что сбрасывает `social_error`.
    fn apply_social(&mut self, response: BridgeResponse) {
        self.social_error = None;
        match response {
            BridgeResponse::Friends(friends) => self.friends = friends,
            BridgeResponse::FriendRequests(requests) => self.friend_requests = requests,
            BridgeResponse::Users(users) => self.search_results = users,
            BridgeResponse::DmChannels(channels) => self.dm_channels = channels,
            BridgeResponse::DmChannel(channel) => {
                let id = channel.id.clone();
                self.opened_dm = Some(id.clone());
                // Сервер не обязан присылать создателю `dm_channel_created`,
                // а сайдбар и лента читают личные каналы из кэша.
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_dm_channel((*channel).clone());
                }
                if !self.dm_channels.iter().any(|known| known.id == id) {
                    self.dm_channels.push(*channel);
                }
            }
            BridgeResponse::Explore(page) => {
                self.explore = Some(page);
                self.explore_loading = false;
            }
            BridgeResponse::Space(space) => {
                let id = space.space.id.clone();
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_space(*space);
                }
                self.opened_space = Some(id);
                // Каталог помечает вступившие пространства: обновим его.
                self.explore = None;
            }
            BridgeResponse::Profile(user) => {
                if let Some(cache) = self.cache.as_mut() {
                    let _ = bs_core::apply(cache, ServerEvent::UserUpdated { user: *user });
                }
                self.info = Some(tr("Профиль сохранён").to_owned());
            }
            BridgeResponse::JoinRequested => {
                self.info = Some(tr("Заявка отправлена").to_owned());
            }
            BridgeResponse::Done => {
                self.request(BridgeRequest::Friends);
                self.request(BridgeRequest::FriendRequests);
                self.request(BridgeRequest::DmChannels);
            }
            BridgeResponse::Failed(message) => {
                self.explore_loading = false;
                self.social_error = Some(message);
            }
            _ => {}
        }
    }

    fn apply_group_action(&mut self, event: GroupEvent, channel: Option<Box<bs_types::DmChannel>>) {
        if let Some(cache) = self.cache.as_mut() {
            match (&event.op, &event.result) {
                (GroupOp::Create(_), Ok(())) => {
                    if let Some(channel) = channel {
                        cache.upsert_dm_channel(*channel);
                    }
                }
                (GroupOp::Leave, Ok(())) => {
                    cache.remove_dm_channel(&event.dm_id);
                }
                _ => {}
            }
        }
        self.group_in.push(event);
    }

    /// Ответы, которые экраны заберут сами: складываем в очереди.
    fn enqueue(&mut self, response: BridgeResponse) {
        match response {
            BridgeResponse::Image { path, result } => self.images_ready.push((path, result)),
            BridgeResponse::Compose(event) => self.compose_in.push(event),
            BridgeResponse::Admin(event) => self.admin_in.push(event),
            BridgeResponse::UserProfile(event) => self.profile_in.push(event),
            BridgeResponse::VoiceToken(event) => self.voice_in.push(event),
            BridgeResponse::VoiceBitrate(target, result) => self.bitrate_in.push((target, result)),
            BridgeResponse::Extras(super::extras::ExtrasEvent::Unsupported(op)) => {
                self.bookmarks_unsupported(op);
            }
            BridgeResponse::Extras(event) => self.extras.apply(event),
            BridgeResponse::SpaceAction(event) => self.apply_space_event(event),
            BridgeResponse::Directory(result) => self.directory = Some(result),
            BridgeResponse::InstanceInfo(result) => self.instance_info = Some(result),
            BridgeResponse::Raw { tag, result } => self.raw_ready.push((tag, result)),
            BridgeResponse::Download { id, state } => self.downloads_ready.push((id, state)),
            BridgeResponse::Credential { origin, result } => self.issued.push((origin, result)),
            BridgeResponse::Peering { origin, result } => self.peering.push((origin, result)),
            BridgeResponse::Synced => self.synced = true,
            BridgeResponse::Registry(result) => self.registry_in = Some(result),
            BridgeResponse::RegistrySaved(result) => self.saved_in.push(result),
            BridgeResponse::IdentityDeleted(result) => self.deleted_in.push(result),
            BridgeResponse::Adopted(result) => self.adopted_in = Some(result),
            _ => {}
        }
    }
}
