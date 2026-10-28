//! Применение WebSocket-событий к кэшу.
//!
//! [`apply`] — чистая функция над [`Cache`]: меняет кэш на месте и сообщает,
//! изменилось ли что-то. Неизвестные и «рефетч-события» возвращают `false`.
//!
//! Всё, что зависит от времени (сейчас — только «печатает…»), берёт момент
//! из параметра [`apply_at`]; [`apply`] подставляет системное время.

use bs_types::{
    ChannelId, DmChannelLastMessage, DmMessageWithUser, MessageId, MessageWithUser,
    PresenceIdentity, Reaction, ReadState, ServerEvent, SpaceVoiceState, User, UserId, UserStatus,
    VoiceStateAction, VoiceUserState,
};

use crate::state::{Cache, TYPING_TTL_MS, TypingEntry, insert_sorted};

/// Применяет событие сервера к кэшу, используя системное время.
///
/// Возвращает `true`, если состояние изменилось и подписчикам стоит обновиться.
/// События, требующие перезапроса (`SpaceAccessChanged`, `FederationPeersChanged`,
/// неизвестные), возвращают `false`.
#[must_use]
pub fn apply(cache: &mut Cache, event: ServerEvent) -> bool {
    apply_at(cache, event, now_ms())
}

/// Применяет событие сервера к кэшу на момент `now_ms` (epoch-мс).
///
/// Явный момент времени делает применение событий детерминированным:
/// «печатает…» живёт [`TYPING_TTL_MS`] от последнего события.
#[must_use]
pub fn apply_at(cache: &mut Cache, event: ServerEvent, now_ms: i64) -> bool {
    if let ServerEvent::Ready(payload) = event {
        *cache = Cache::from_ready(*payload);
        return true;
    }
    match categorize(&event) {
        Category::Messages => try_messages(cache, event),
        Category::Typing => try_typing(cache, event, now_ms),
        Category::Presence => try_presence(cache, event),
        Category::Spaces => try_spaces(cache, event),
        Category::Channels => try_channels(cache, event),
        Category::JoinRequests => try_join_requests(cache, event),
        Category::Voice => try_voice(cache, event),
        Category::Unread => try_unread(cache, event),
        Category::Dm => try_dm(cache, event),
        Category::Federation => try_federation(cache, event),
        Category::Friends => try_friends(cache, event),
        Category::Ignored => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Messages,
    Typing,
    Presence,
    Spaces,
    Channels,
    JoinRequests,
    Voice,
    Unread,
    Dm,
    Federation,
    Friends,
    Ignored,
}

fn categorize(event: &ServerEvent) -> Category {
    match event {
        ServerEvent::MessageCreated { .. }
        | ServerEvent::MessageUpdated { .. }
        | ServerEvent::MessageDeleted { .. }
        | ServerEvent::EmbedsResolved { .. }
        | ServerEvent::ReactionAdded { .. }
        | ServerEvent::ReactionRemoved { .. } => Category::Messages,
        ServerEvent::Typing { .. } => Category::Typing,
        ServerEvent::PresenceUpdate { .. } => Category::Presence,
        ServerEvent::MemberJoined { .. }
        | ServerEvent::MemberUpdated { .. }
        | ServerEvent::MemberLeft { .. }
        | ServerEvent::MemberBanned { .. }
        | ServerEvent::SpaceUpdated { .. }
        | ServerEvent::SpaceAccessChanged { .. }
        | ServerEvent::UserUpdated { .. } => Category::Spaces,
        ServerEvent::ChannelCreated { .. }
        | ServerEvent::ChannelUpdated { .. }
        | ServerEvent::ChannelDeleted { .. }
        | ServerEvent::CategoryCreated { .. }
        | ServerEvent::CategoryUpdated { .. }
        | ServerEvent::CategoryDeleted { .. }
        | ServerEvent::ChannelLayoutUpdated { .. }
        | ServerEvent::SpaceLayoutUpdated { .. } => Category::Channels,
        ServerEvent::JoinRequestAccepted { .. } => Category::JoinRequests,
        ServerEvent::VoiceStateUpdate { .. }
        | ServerEvent::VoiceStatusUpdate { .. }
        | ServerEvent::SpaceVoiceState { .. }
        | ServerEvent::VoiceSpaceMuted { .. }
        | ServerEvent::VoiceSpaceDeafened { .. }
        | ServerEvent::VoicePermissionMuted { .. }
        | ServerEvent::VoiceMoved { .. }
        | ServerEvent::VoiceDisconnected { .. } => Category::Voice,
        ServerEvent::ChannelAck { .. }
        | ServerEvent::MarkUnread { .. }
        | ServerEvent::ChannelUnreadCount { .. } => Category::Unread,
        ServerEvent::DmChannelCreated { .. }
        | ServerEvent::DmChannelClosed { .. }
        | ServerEvent::DmChannelUpdated { .. }
        | ServerEvent::DmMessageCreated { .. }
        | ServerEvent::DmMessageUpdated { .. }
        | ServerEvent::DmMessageDeleted { .. }
        | ServerEvent::DmEmbedsResolved { .. }
        | ServerEvent::DmMemberAdded { .. }
        | ServerEvent::DmMemberRemoved { .. } => Category::Dm,
        ServerEvent::FederationPeerRejected { .. }
        | ServerEvent::FederationPeerActive { .. }
        | ServerEvent::FederationApprovalRequestReceived { .. } => Category::Federation,
        ServerEvent::FriendRequestReceived { .. }
        | ServerEvent::FriendRequestSent { .. }
        | ServerEvent::FriendRequestAccepted { .. }
        | ServerEvent::FriendRequestCancelled { .. }
        | ServerEvent::FriendRequestDeclined { .. }
        | ServerEvent::FriendRequestRelayFailed { .. }
        | ServerEvent::FriendRemoved { .. } => Category::Friends,
        _ => Category::Ignored,
    }
}

fn try_messages(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Сообщения ────────────────────────────────────────────────────
        ServerEvent::MessageCreated { message } => {
            let channel_id = message.message.channel_id.clone();
            let message_id = message.message.id.clone();
            upsert_message(cache, message);
            set_last_message_id(cache, &channel_id, message_id);
            true
        }
        ServerEvent::MessageUpdated { message } => {
            upsert_message(cache, message);
            true
        }
        ServerEvent::MessageDeleted {
            message_id,
            channel_id,
        } => delete_message(cache, &channel_id, &message_id),
        ServerEvent::EmbedsResolved {
            message_id, embeds, ..
        } => match find_message_mut(cache, &message_id) {
            Some(msg) => {
                msg.embeds = embeds;
                true
            }
            None => false,
        },

        // ─── Реакции ──────────────────────────────────────────────────────
        ServerEvent::ReactionAdded {
            message_id,
            reaction,
        } => match reactions_mut(cache, &message_id) {
            Some(reactions) => {
                let already = reactions.iter().any(|r| {
                    r.id == reaction.id
                        || (r.user_id == reaction.user_id && r.emoji == reaction.emoji)
                });
                if already {
                    return false;
                }
                reactions.push(reaction);
                true
            }
            None => false,
        },
        ServerEvent::ReactionRemoved {
            message_id,
            user_id,
            emoji,
        } => reactions_mut(cache, &message_id).is_some_and(|reactions| {
            let before = reactions.len();
            reactions.retain(|r| !(r.user_id == user_id && r.emoji == emoji));
            reactions.len() != before
        }),

        _ => false,
    }
}

fn try_typing(cache: &mut Cache, event: ServerEvent, now_ms: i64) -> bool {
    match event {
        // ─── «Печатает…» ──────────────────────────────────────────────────
        ServerEvent::Typing {
            channel_id,
            user_id,
            username,
        } => {
            let expires_at = now_ms.saturating_add(TYPING_TTL_MS);
            let entries = cache.typing.entry(channel_id).or_default();
            match entries.iter_mut().find(|e| e.user_id == user_id) {
                Some(existing) => {
                    existing.username = username;
                    existing.expires_at = expires_at;
                }
                None => entries.push(TypingEntry {
                    user_id,
                    username,
                    expires_at,
                }),
            }
            true
        }

        _ => false,
    }
}

fn try_presence(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Присутствие ──────────────────────────────────────────────────
        ServerEvent::PresenceUpdate {
            user_id,
            status,
            activities,
            home_user_id,
            home_instance,
        } => {
            let mut changed = set_user_status(cache, &user_id, status);
            if let Some(activities) = activities {
                if activities.is_empty() {
                    changed |= cache.user_activities.remove(&user_id).is_some();
                } else {
                    cache.user_activities.insert(user_id.clone(), activities);
                    changed = true;
                }
            }
            if home_user_id.is_some() || home_instance.is_some() {
                cache.user_activity_identities.insert(
                    user_id,
                    PresenceIdentity {
                        home_user_id,
                        home_instance,
                    },
                );
                changed = true;
            }
            changed
        }

        _ => false,
    }
}

fn try_spaces(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Пространства и участники ─────────────────────────────────────
        ServerEvent::MemberJoined { space_id, member }
        | ServerEvent::MemberUpdated { space_id, member } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            match space
                .members
                .iter_mut()
                .find(|m| m.user.id == member.user.id)
            {
                Some(existing) => *existing = member,
                None => space.members.push(member),
            }
            true
        }
        ServerEvent::MemberLeft { space_id, user_id } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            let before = space.members.len();
            space.members.retain(|m| m.user.id != user_id);
            space.members.len() != before
        }
        ServerEvent::SpaceUpdated { space } => match cache.spaces.get_mut(&space.id) {
            Some(existing) => {
                existing.space = space;
                true
            }
            None => false,
        },
        ServerEvent::UserUpdated { user } => update_user(cache, &user),

        _ => false,
    }
}

fn try_channels(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Каналы и категории ───────────────────────────────────────────
        ServerEvent::ChannelCreated { channel, space_id }
        | ServerEvent::ChannelUpdated { channel, space_id } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            match space.channels.iter_mut().find(|c| c.id == channel.id) {
                Some(existing) => *existing = channel,
                None => space.channels.push(channel),
            }
            true
        }
        ServerEvent::ChannelDeleted {
            channel_id,
            space_id,
        } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            let before = space.channels.len();
            space.channels.retain(|c| c.id != channel_id);
            let removed = space.channels.len() != before;
            cache.messages.remove(&channel_id);
            cache.typing.remove(&channel_id);
            cache.unread_counts.remove(&channel_id);
            cache.read_states.remove(&channel_id);
            cache.voice_states.remove(&channel_id);
            cache.voice_channel_elapsed_seconds.remove(&channel_id);
            removed
        }
        ServerEvent::CategoryCreated { category, space_id }
        | ServerEvent::CategoryUpdated { category, space_id } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            match space.categories.iter_mut().find(|c| c.id == category.id) {
                Some(existing) => *existing = category,
                None => space.categories.push(category),
            }
            true
        }
        ServerEvent::CategoryDeleted {
            category_id,
            space_id,
        } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            let before = space.categories.len();
            space.categories.retain(|c| c.id != category_id);
            space.categories.len() != before
        }
        ServerEvent::ChannelLayoutUpdated {
            space_id,
            channels,
            categories,
        } => {
            let Some(space) = cache.spaces.get_mut(&space_id) else {
                return false;
            };
            space.channels = channels;
            space.categories = categories;
            true
        }
        ServerEvent::SpaceLayoutUpdated {
            layout,
            folders,
            updated_at,
        } => {
            cache.space_layout = layout;
            cache.folders = folders;
            cache.layout_updated_at = updated_at;
            true
        }

        _ => false,
    }
}

fn try_join_requests(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Заявки ───────────────────────────────────────────────────────
        ServerEvent::JoinRequestAccepted { space, .. } => {
            cache.spaces.insert(space.space.id.clone(), space);
            true
        }

        _ => false,
    }
}

fn try_voice(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Голос ────────────────────────────────────────────────────────
        ServerEvent::VoiceStateUpdate {
            channel_id,
            user_id,
            action,
            channel_elapsed_seconds,
        } => match action {
            VoiceStateAction::Join => {
                let users = cache.voice_states.entry(channel_id.clone()).or_default();
                if users.contains(&user_id) {
                    return false;
                }
                users.push(user_id);
                if let Some(secs) = channel_elapsed_seconds {
                    cache.voice_channel_elapsed_seconds.insert(channel_id, secs);
                }
                true
            }
            VoiceStateAction::Leave => {
                let removed = remove_from_voice(cache, &channel_id, &user_id);
                cache.voice_user_states.remove(&user_id);
                removed
            }
        },
        ServerEvent::VoiceStatusUpdate {
            user_id,
            is_muted,
            is_deafened,
            is_camera_on,
            is_screen_sharing,
            ..
        } => {
            cache.voice_user_states.insert(
                user_id,
                VoiceUserState {
                    is_muted,
                    is_deafened,
                    is_camera_on,
                    is_screen_sharing,
                },
            );
            true
        }
        ServerEvent::SpaceVoiceState {
            voice_states,
            voice_channel_elapsed_seconds,
            voice_user_states,
            ..
        } => {
            cache.voice_states = voice_states;
            cache.voice_channel_elapsed_seconds = voice_channel_elapsed_seconds;
            cache.voice_user_states = voice_user_states;
            true
        }
        other => try_voice_space(cache, other),
    }
}

fn try_voice_space(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        ServerEvent::VoiceSpaceMuted {
            user_id,
            space_id,
            muted,
            ..
        } => {
            if user_id != cache.me.id {
                return false;
            }
            let entry = cache
                .space_voice_states
                .entry(space_id)
                .or_insert(empty_space_voice_state());
            entry.space_muted = muted;
            true
        }
        ServerEvent::VoiceSpaceDeafened {
            user_id,
            space_id,
            deafened,
            ..
        } => {
            if user_id != cache.me.id {
                return false;
            }
            let entry = cache
                .space_voice_states
                .entry(space_id)
                .or_insert(empty_space_voice_state());
            entry.space_deafened = deafened;
            true
        }
        ServerEvent::VoicePermissionMuted {
            user_id,
            space_id,
            muted,
        } => {
            if user_id != cache.me.id {
                return false;
            }
            let entry = cache
                .space_voice_states
                .entry(space_id)
                .or_insert(empty_space_voice_state());
            entry.permission_muted = Some(muted);
            true
        }
        ServerEvent::VoiceMoved {
            user_id,
            old_channel_id,
            new_channel_id,
        } => {
            let _ = remove_from_voice(cache, &old_channel_id, &user_id);
            let users = cache.voice_states.entry(new_channel_id).or_default();
            if !users.contains(&user_id) {
                users.push(user_id);
            }
            true
        }
        ServerEvent::VoiceDisconnected {
            user_id,
            channel_id,
            ..
        } => {
            let removed = remove_from_voice(cache, &channel_id, &user_id);
            cache.voice_user_states.remove(&user_id);
            removed
        }

        _ => false,
    }
}

fn try_unread(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Непрочитанное ────────────────────────────────────────────────
        ServerEvent::ChannelAck {
            channel_id,
            message_id,
        } => {
            let entry = cache
                .read_states
                .entry(channel_id.clone())
                .or_insert_with(|| ReadState {
                    channel_id: channel_id.clone(),
                    last_read_message_id: None,
                });
            entry.last_read_message_id = Some(message_id);
            cache.unread_counts.insert(channel_id, 0);
            true
        }
        ServerEvent::MarkUnread { channel_id, .. } => {
            let count = cache.unread_counts.entry(channel_id).or_insert(0);
            if *count == 0 {
                *count = 1;
                true
            } else {
                false
            }
        }
        ServerEvent::ChannelUnreadCount { counts } => {
            cache.unread_counts = counts;
            true
        }

        _ => false,
    }
}

fn try_dm(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Личные сообщения ─────────────────────────────────────────────
        ServerEvent::DmChannelCreated { dm_channel } => {
            cache.dm_channels.insert(dm_channel.id.clone(), dm_channel);
            true
        }
        ServerEvent::DmChannelClosed { dm_channel_id } => {
            cache.dm_channels.remove(&dm_channel_id).is_some()
        }
        ServerEvent::DmChannelUpdated {
            dm_channel_id,
            name,
            icon,
        } => match cache.dm_channels.get_mut(&dm_channel_id) {
            Some(dm) => {
                if name.is_some() {
                    dm.name = name;
                }
                if icon.is_some() {
                    dm.icon = icon;
                }
                true
            }
            None => false,
        },
        ServerEvent::DmMessageCreated { message } | ServerEvent::DmMessageUpdated { message } => {
            upsert_dm_message(cache, message.clone());
            if let Some(dm) = cache.dm_channels.get_mut(&message.message.dm_channel_id) {
                dm.last_message = Some(DmChannelLastMessage::Full(Box::new(message)));
            }
            true
        }
        ServerEvent::DmMessageDeleted {
            message_id,
            dm_channel_id,
        } => cache
            .dm_messages
            .get_mut(&dm_channel_id)
            .is_some_and(|list| {
                let before = list.len();
                list.retain(|m| m.message.id != message_id);
                list.len() != before
            }),
        ServerEvent::DmEmbedsResolved {
            message_id,
            dm_channel_id,
            embeds,
        } => cache
            .dm_messages
            .get_mut(&dm_channel_id)
            .and_then(|list| list.iter_mut().find(|m| m.message.id == message_id))
            .is_some_and(|msg| {
                msg.embeds = embeds;
                true
            }),
        ServerEvent::DmMemberAdded {
            dm_channel_id,
            user,
        } => match cache.dm_channels.get_mut(&dm_channel_id) {
            Some(dm) => {
                if dm.members.iter().any(|m| m.id == user.id) {
                    return false;
                }
                dm.members.push(user);
                true
            }
            None => false,
        },
        ServerEvent::DmMemberRemoved {
            dm_channel_id,
            user_id,
        } => match cache.dm_channels.get_mut(&dm_channel_id) {
            Some(dm) => {
                let before = dm.members.len();
                dm.members.retain(|m| m.id != user_id);
                dm.members.len() != before
            }
            None => false,
        },

        _ => false,
    }
}

fn try_federation(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Федерация ────────────────────────────────────────────────────
        ServerEvent::FederationPeerRejected { peer_origin, .. } => {
            if cache.rejected_peer_origins.contains(&peer_origin) {
                return false;
            }
            cache.rejected_peer_origins.push(peer_origin);
            true
        }
        ServerEvent::FederationPeerActive { peer_origin } => {
            if cache.active_peer_origins.contains(&peer_origin) {
                return false;
            }
            cache.active_peer_origins.push(peer_origin);
            true
        }
        ServerEvent::FederationApprovalRequestReceived { origin, .. } => {
            cache.pending_approval_count = cache.pending_approval_count.saturating_add(1);
            if !cache.awaiting_approval_peer_origins.contains(&origin) {
                cache.awaiting_approval_peer_origins.push(origin);
            }
            true
        }
        _ => false,
    }
}

// ─── Вспомогательные функции ──────────────────────────────────────────────

fn try_friends(cache: &mut Cache, event: ServerEvent) -> bool {
    match event {
        // ─── Друзья ───────────────────────────────────────────────────────
        ServerEvent::FriendRequestReceived { request }
        | ServerEvent::FriendRequestSent { request } => {
            if cache.friend_requests.get(&request.id) == Some(&request) {
                return false;
            }
            cache.friend_requests.insert(request.id.clone(), request);
            true
        }
        ServerEvent::FriendRequestAccepted { friend, request_id } => {
            let request_removed = cache.friend_requests.remove(&request_id).is_some();
            if cache.friends.get(&friend.id) == Some(&friend) {
                return request_removed;
            }
            cache.user_statuses.insert(friend.id.clone(), friend.status);
            cache.friends.insert(friend.id.clone(), friend);
            true
        }
        ServerEvent::FriendRequestCancelled { request_id, .. }
        | ServerEvent::FriendRequestDeclined { request_id, .. } => {
            cache.friend_requests.remove(&request_id).is_some()
        }
        ServerEvent::FriendRemoved { user_id } => cache.friends.remove(&user_id).is_some(),

        _ => false,
    }
}

/// Текущий момент в epoch-миллисекундах (0, если часы ушли до эпохи).
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        i64::try_from(d.as_millis()).map_or(i64::MAX, |ms| ms)
    })
}

const fn empty_space_voice_state() -> SpaceVoiceState {
    SpaceVoiceState {
        space_muted: false,
        space_deafened: false,
        permission_muted: None,
    }
}

fn upsert_message(cache: &mut Cache, message: MessageWithUser) {
    let list = cache
        .messages
        .entry(message.message.channel_id.clone())
        .or_default();
    if let Some(pos) = list.iter().position(|m| m.message.id == message.message.id) {
        list[pos] = message;
        return;
    }
    insert_sorted(list, message, |m| {
        (m.message.created_at, m.message.id.as_str())
    });
}

fn upsert_dm_message(cache: &mut Cache, message: DmMessageWithUser) {
    let list = cache
        .dm_messages
        .entry(message.message.dm_channel_id.clone())
        .or_default();
    if let Some(pos) = list.iter().position(|m| m.message.id == message.message.id) {
        list[pos] = message;
        return;
    }
    insert_sorted(list, message, |m| {
        (m.message.created_at, m.message.id.as_str())
    });
}

/// Реакции сообщения: и канала пространства, и личного канала.
fn reactions_mut<'a>(
    cache: &'a mut Cache,
    message_id: &MessageId,
) -> Option<&'a mut Vec<Reaction>> {
    if let Some(msg) = cache
        .messages
        .values_mut()
        .flat_map(|v| v.iter_mut())
        .find(|m| m.message.id == *message_id)
    {
        return Some(&mut msg.reactions);
    }
    cache
        .dm_messages
        .values_mut()
        .flat_map(|v| v.iter_mut())
        .find(|m| m.message.id == *message_id)
        .map(|m| &mut m.reactions)
}

fn set_last_message_id(cache: &mut Cache, channel_id: &ChannelId, message_id: MessageId) {
    for space in cache.spaces.values_mut() {
        if let Some(channel) = space
            .channels
            .iter_mut()
            .find(|c| c.id == channel_id.as_str())
        {
            channel.last_message_id = Some(message_id);
            return;
        }
    }
}

fn delete_message(cache: &mut Cache, channel_id: &ChannelId, message_id: &MessageId) -> bool {
    let Some(list) = cache.messages.get_mut(channel_id) else {
        return false;
    };
    let before = list.len();
    list.retain(|m| m.message.id != message_id.as_str());
    list.len() != before
}

fn find_message_mut<'a>(
    cache: &'a mut Cache,
    message_id: &MessageId,
) -> Option<&'a mut MessageWithUser> {
    cache
        .messages
        .values_mut()
        .flat_map(|v| v.iter_mut())
        .find(|m| m.message.id == *message_id)
}

fn remove_from_voice(cache: &mut Cache, channel_id: &ChannelId, user_id: &UserId) -> bool {
    let Some(users) = cache.voice_states.get_mut(channel_id) else {
        return false;
    };
    let before = users.len();
    users.retain(|u| u != user_id);
    if users.len() == before {
        return false;
    }
    if users.is_empty() {
        cache.voice_states.remove(channel_id);
        cache.voice_channel_elapsed_seconds.remove(channel_id);
    }
    true
}

fn set_user_status(cache: &mut Cache, user_id: &UserId, status: UserStatus) -> bool {
    let mut changed = if cache.me.id == *user_id && cache.me.status != status {
        cache.me.status = status;
        true
    } else {
        false
    };
    for space in cache.spaces.values_mut() {
        for member in &mut space.members {
            if member.user.id == *user_id && member.user.status != status {
                member.user.status = status;
                changed = true;
            }
        }
    }
    for dm in cache.dm_channels.values_mut() {
        for user in &mut dm.members {
            if user.id == *user_id && user.status != status {
                user.status = status;
                changed = true;
            }
        }
    }
    if cache.user_statuses.insert(user_id.clone(), status) != Some(status) {
        changed = true;
    }
    changed
}

fn update_user(cache: &mut Cache, user: &User) -> bool {
    let id = user.id.clone();
    if cache.me.id == id {
        cache.me = user.clone();
    }
    for space in cache.spaces.values_mut() {
        for member in &mut space.members {
            if member.user.id == id {
                member.user = user.clone();
            }
        }
    }
    for dm in cache.dm_channels.values_mut() {
        for member in &mut dm.members {
            if member.id == id {
                *member = user.clone();
            }
        }
    }
    cache.user_statuses.insert(id, user.status);
    true
}

#[cfg(test)]
#[path = "apply/tests.rs"]
mod tests;
