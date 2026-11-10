//! Сквозной сценарий против настоящего сервера Backspace.
//!
//! Без `BS_CONTRACT_URL` тест сразу завершается. С ним — регистрирует трёх
//! пользователей и проходит основные функции: пространства, каналы, роли,
//! сообщения, реакции, ответы, друзья, ЛС, групповые беседы, уведомления,
//! профиль, приглашения. Каждое событие WebSocket и выбранные REST-ответы
//! разбираются строго; итоговый отчёт пишется в `target/contract-report.txt`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bs_api::{
    ApiClient, CreateDmRequest, CreateSpaceRequest, FriendRequestDecision, GroupMember,
    GroupUpdate, MessageSearchQuery, NewChannel, NewInvite, NewRole, RegisterRequest,
    SendFriendRequest, SendMessageBody, SpaceUpdate, UpdateProfileRequest, UsernameCheck,
};
use bs_contract::{Findings, StrictSocket, check_typed, server_url, unique};
use bs_types::{ClientEvent, ServerEvent, UpdateNotificationSettingRequest};
use serde_json::Value;

struct Account {
    api: ApiClient,
    id: String,
    token: String,
    username: String,
}

async fn register(base: &str, prefix: &str) -> Account {
    let mut api = ApiClient::home(base).unwrap();
    let username = unique(prefix);
    assert_eq!(
        api.check_username(&username).await.unwrap(),
        UsernameCheck::Available,
        "имя должно быть свободно"
    );
    let auth = api
        .register(&RegisterRequest {
            username: username.clone(),
            password: "contract-password-1".into(),
            display_name: Some(format!("{prefix} display")),
            avatar_color: Some("mint".into()),
            ..RegisterRequest::default()
        })
        .await
        .unwrap_or_else(|err| panic!("регистрация {username}: {err}"));
    api.set_token(Some(auth.token.clone()));
    Account {
        api,
        id: auth.user.id.clone(),
        token: auth.token,
        username,
    }
}

/// Сырой GET для сверки полей REST-ответа с типом клиента.
async fn raw_get(base: &str, token: &str, path: &str) -> Value {
    let client = reqwest::Client::new();
    client
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|err| panic!("GET {path}: {err}"))
        .json()
        .await
        .unwrap_or_else(|err| panic!("GET {path} json: {err}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn full_client_scenario_against_real_server() {
    let Some(base) = server_url() else {
        eprintln!("BS_CONTRACT_URL не задан — контрактный тест пропущен");
        return;
    };
    let mut findings = Findings::default();

    // ─── Инстанс и регистрация ─────────────────────────────────────────────
    let info = ApiClient::home(&base)
        .unwrap()
        .instance_info()
        .await
        .unwrap();
    assert!(
        info.registration_open,
        "для стенда нужна открытая регистрация"
    );
    let alice = register(&base, "alice").await;
    let bob = register(&base, "bob").await;
    let carol = register(&base, "carol").await;
    assert_eq!(
        bob.api.check_username(&alice.username).await.unwrap(),
        UsernameCheck::Taken
    );
    let me = alice.api.me().await.unwrap();
    assert_eq!(me.username, alice.username);

    let (mut alice_ws, ready) = StrictSocket::connect(&base, &alice.token).await.unwrap();
    let ServerEvent::Ready(ready) = ready else {
        unreachable!()
    };
    assert_eq!(ready.user.id, alice.id);
    let (mut bob_ws, _) = StrictSocket::connect(&base, &bob.token).await.unwrap();
    let (mut carol_ws, _) = StrictSocket::connect(&base, &carol.token).await.unwrap();

    // ─── Пространство, каналы, приглашение ─────────────────────────────────
    let space = alice
        .api
        .create_space(&CreateSpaceRequest {
            name: unique("space"),
            visibility: None,
            description: Some("контрактный тест".into()),
            avatar_color: Some("sky".into()),
        })
        .await
        .unwrap();
    let space_id = space.id.clone();
    let space = alice.api.space(&space_id).await.unwrap();
    let text_channel = space
        .channels
        .iter()
        .find(|c| c.channel_type == bs_types::ChannelType::Text)
        .expect("в новом пространстве есть текстовый канал")
        .id
        .clone();
    alice
        .api
        .create_category(&space_id, "Категория")
        .await
        .unwrap();
    alice
        .api
        .create_channel(
            &space_id,
            &NewChannel {
                name: "второй".into(),
                kind: "text".into(),
                topic: Some("тема".into()),
                category_id: None,
            },
        )
        .await
        .unwrap();
    alice_ws
        .wait_for("channel_created", |e| {
            matches!(e, ServerEvent::ChannelCreated { .. }).then_some(())
        })
        .await
        .unwrap();
    alice
        .api
        .update_space(
            &space_id,
            &SpaceUpdate {
                description: Some("обновлено".into()),
                ..SpaceUpdate::default()
            },
        )
        .await
        .unwrap();

    let code = alice.api.space_invite_code(&space_id).await.unwrap();
    assert_eq!(bob.api.join_by_code(&code).await.unwrap(), space_id);
    assert_eq!(carol.api.join_by_code(&code).await.unwrap(), space_id);
    alice_ws
        .wait_for("member_joined", |e| {
            matches!(e, ServerEvent::MemberJoined { .. }).then_some(())
        })
        .await
        .unwrap();
    let full = bob.api.space(&space_id).await.unwrap();
    assert!(full.members.iter().any(|m| m.user.id == bob.id));

    // ─── Роли ──────────────────────────────────────────────────────────────
    let role = alice
        .api
        .create_role(
            &space_id,
            &NewRole {
                name: "Модератор".into(),
                color: "#86efac".into(),
                permissions: None,
            },
        )
        .await
        .unwrap();
    alice
        .api
        .add_member_role(&space_id, &bob.id, &role.id)
        .await
        .unwrap();
    alice
        .api
        .remove_member_role(&space_id, &bob.id, &role.id)
        .await
        .unwrap();
    alice.api.delete_role(&space_id, &role.id).await.unwrap();

    // ─── Сообщения через WebSocket ─────────────────────────────────────────
    alice_ws
        .send(&ClientEvent::MessageCreate {
            channel_id: text_channel.clone(),
            content: "привет, **мир**".into(),
            reply_to_id: None,
        })
        .await
        .unwrap();
    let first = bob_ws
        .wait_for("message_created", |e| match e {
            ServerEvent::MessageCreated { message } => Some(message.clone()),
            _ => None,
        })
        .await
        .unwrap();
    assert_eq!(first.message.content.as_deref(), Some("привет, **мир**"));

    bob_ws
        .send(&ClientEvent::TypingStart {
            channel_id: text_channel.clone(),
        })
        .await
        .unwrap();
    bob_ws
        .send(&ClientEvent::MessageCreate {
            channel_id: text_channel.clone(),
            content: format!("ответ <@{}>", alice.id),
            reply_to_id: Some(first.message.id.clone()),
        })
        .await
        .unwrap();
    let reply = alice_ws
        .wait_for("reply", |e| match e {
            ServerEvent::MessageCreated { message } if message.user.id == bob.id => {
                Some(message.clone())
            }
            _ => None,
        })
        .await
        .unwrap();
    assert_eq!(reply.message.reply_to_id.as_ref(), Some(&first.message.id));

    // REST-отправка тоже должна работать.
    let rest_message = carol
        .api
        .send_channel_message(
            &text_channel,
            &SendMessageBody {
                content: "через REST".into(),
                attachments: Vec::new(),
                reply_to_id: None,
            },
        )
        .await
        .unwrap();

    alice_ws
        .send(&ClientEvent::MessageEdit {
            message_id: first.message.id.clone(),
            content: "привет, мир (изменено)".into(),
        })
        .await
        .unwrap();
    bob_ws
        .wait_for("message_updated", |e| {
            matches!(e, ServerEvent::MessageUpdated { .. }).then_some(())
        })
        .await
        .unwrap();

    bob_ws
        .send(&ClientEvent::ReactionAdd {
            message_id: first.message.id.clone(),
            emoji: "👍".into(),
        })
        .await
        .unwrap();
    alice_ws
        .wait_for("reaction_added", |e| {
            matches!(e, ServerEvent::ReactionAdded { .. }).then_some(())
        })
        .await
        .unwrap();
    bob_ws
        .send(&ClientEvent::ReactionRemove {
            message_id: first.message.id.clone(),
            emoji: "👍".into(),
        })
        .await
        .unwrap();
    alice_ws
        .wait_for("reaction_removed", |e| {
            matches!(e, ServerEvent::ReactionRemoved { .. }).then_some(())
        })
        .await
        .unwrap();

    alice_ws
        .send(&ClientEvent::ChannelAck {
            channel_id: text_channel.clone(),
            message_id: rest_message.message.id.clone(),
        })
        .await
        .unwrap();
    carol_ws
        .send(&ClientEvent::MessageDelete {
            message_id: rest_message.message.id.clone(),
        })
        .await
        .unwrap();
    alice_ws
        .wait_for("message_deleted", |e| {
            matches!(e, ServerEvent::MessageDeleted { .. }).then_some(())
        })
        .await
        .unwrap();

    let page = alice
        .api
        .messages(&text_channel, None, Some(50))
        .await
        .unwrap();
    assert!(page.messages.len() >= 2);
    let raw = raw_get(
        &base,
        &alice.token,
        &format!("/api/channels/{text_channel}/messages?limit=50"),
    )
    .await;
    if let Some(items) = raw
        .as_array()
        .or_else(|| raw.get("messages").and_then(Value::as_array))
    {
        for item in items {
            check_typed::<bs_types::MessageWithUser>("rest.message", item, &mut findings).unwrap();
        }
    }
    let search = alice
        .api
        .search_channel_messages(
            &text_channel,
            &MessageSearchQuery {
                text: Some("привет".into()),
                ..MessageSearchQuery::default()
            },
        )
        .await
        .unwrap();
    assert!(search.total_count >= 1);

    // ─── Друзья ────────────────────────────────────────────────────────────
    let request_id = alice
        .api
        .send_friend_request(&SendFriendRequest {
            username: Some(bob.username.clone()),
            ..SendFriendRequest::default()
        })
        .await
        .unwrap();
    bob_ws
        .wait_for("friend_request_received", |e| {
            matches!(e, ServerEvent::FriendRequestReceived { .. }).then_some(())
        })
        .await
        .unwrap();
    bob.api
        .respond_friend_request(&request_id, FriendRequestDecision::Accept)
        .await
        .unwrap();
    alice_ws
        .wait_for("friend_request_accepted", |e| {
            matches!(e, ServerEvent::FriendRequestAccepted { .. }).then_some(())
        })
        .await
        .unwrap();
    assert!(
        alice
            .api
            .friends()
            .await
            .unwrap()
            .iter()
            .any(|f| f.id == bob.id)
    );
    let raw = raw_get(&base, &alice.token, "/api/social/friends").await;
    for item in raw.as_array().into_iter().flatten() {
        check_typed::<bs_types::Friend>("rest.friend", item, &mut findings).unwrap();
    }

    // ─── Личные сообщения ──────────────────────────────────────────────────
    let dm = alice
        .api
        .open_dm(&CreateDmRequest {
            user_id: Some(bob.id.clone()),
            ..CreateDmRequest::default()
        })
        .await
        .unwrap();
    alice_ws
        .send(&ClientEvent::DmTypingStart {
            dm_channel_id: dm.id.clone(),
        })
        .await
        .unwrap();
    alice_ws
        .send(&ClientEvent::DmMessageCreate {
            dm_channel_id: dm.id.clone(),
            content: Some("личное".into()),
            attachments: None,
            reply_to_id: None,
        })
        .await
        .unwrap();
    let dm_message = bob_ws
        .wait_for("dm_message_created", |e| match e {
            ServerEvent::DmMessageCreated { message } => Some(message.clone()),
            _ => None,
        })
        .await
        .unwrap();
    bob_ws
        .send(&ClientEvent::ReactionAdd {
            message_id: dm_message.message.id.clone(),
            emoji: "❤️".into(),
        })
        .await
        .unwrap();
    alice_ws
        .send(&ClientEvent::DmMessageEdit {
            message_id: dm_message.message.id.clone(),
            content: "личное (изменено)".into(),
        })
        .await
        .unwrap();
    bob_ws
        .wait_for("dm_message_updated", |e| {
            matches!(e, ServerEvent::DmMessageUpdated { .. }).then_some(())
        })
        .await
        .unwrap();
    let dm_page = bob.api.dm_messages(&dm.id, None, Some(50)).await.unwrap();
    assert!(!dm_page.messages.is_empty());
    let raw = raw_get(&base, &bob.token, "/api/dm").await;
    for item in raw.as_array().into_iter().flatten() {
        check_typed::<bs_types::DmChannel>("rest.dm_channel", item, &mut findings).unwrap();
    }

    // ─── Приглашение в пространство через ЛС ───────────────────────────────
    let sent = alice
        .api
        .send_space_invite(&bs_api::SpaceInviteRequest {
            target: bs_api::InviteTarget::Local {
                user_id: bob.id.clone(),
            },
            space_id: space_id.clone(),
            space_instance_origin: String::new(),
            invite_code: code.clone(),
        })
        .await
        .unwrap();
    let invite = bob_ws
        .wait_for("dm_message_created(space_invite)", |e| match e {
            ServerEvent::DmMessageCreated { message } if message.message.id == sent.message_id => {
                Some(message.clone())
            }
            _ => None,
        })
        .await
        .unwrap();
    assert_eq!(
        invite.message.message_type,
        Some(bs_types::MessageType::System)
    );
    let parsed = bs_types::DmSystemEvent::parse(invite.message.content.as_deref().unwrap())
        .expect("клиент разбирает space_invite сервера");
    assert!(
        matches!(parsed, bs_types::DmSystemEvent::SpaceInvite(ref i) if i.space_id == space_id)
    );

    // ─── Групповая беседа (только с друзьями) ──────────────────────────────
    let carol_request = alice
        .api
        .send_friend_request(&SendFriendRequest {
            username: Some(carol.username.clone()),
            ..SendFriendRequest::default()
        })
        .await
        .unwrap();
    carol
        .api
        .respond_friend_request(&carol_request, FriendRequestDecision::Accept)
        .await
        .unwrap();
    let group = alice
        .api
        .create_group_dm(&[GroupMember::local(&bob.id), GroupMember::local(&carol.id)])
        .await
        .unwrap();
    carol_ws
        .wait_for("dm_channel_created", |e| {
            matches!(e, ServerEvent::DmChannelCreated { .. }).then_some(())
        })
        .await
        .unwrap();
    alice
        .api
        .update_group_dm(
            &group.id,
            &GroupUpdate {
                name: Some(Some("Тройка".into())),
                icon: None,
            },
        )
        .await
        .unwrap();
    carol.api.leave_group_dm(&group.id).await.unwrap();
    // Системные события группы клиент должен понимать все до одного.
    let history = alice
        .api
        .dm_messages(&group.id, None, Some(50))
        .await
        .unwrap();
    let system: Vec<_> = history
        .messages
        .iter()
        .filter(|m| m.message.message_type == Some(bs_types::MessageType::System))
        .collect();
    assert!(system.len() >= 3, "added×2, name_changed, member_removed");
    for message in system {
        let content = message.message.content.as_deref().unwrap_or_default();
        assert!(
            bs_types::DmSystemEvent::parse(content).is_some(),
            "не разобрано системное событие: {content}"
        );
    }

    // ─── Уведомления, профиль, стикеры, GIF ────────────────────────────────
    bob.api
        .update_space_notifications(
            &space_id,
            &UpdateNotificationSettingRequest {
                suppress_everyone: Some(true),
                ..UpdateNotificationSettingRequest::default()
            },
        )
        .await
        .unwrap();
    bob.api.notification_settings().await.unwrap();
    let updated = bob
        .api
        .update_me(&UpdateProfileRequest {
            bio: Some("био из теста".into()),
            custom_status: Some("тестирую".into()),
            ..UpdateProfileRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(updated.bio.as_deref(), Some("био из теста"));
    alice.api.user(&bob.id).await.unwrap();
    alice.api.mutuals(&bob.id).await.unwrap();
    alice.api.stickers().await.unwrap();
    alice.api.gif_enabled().await.unwrap();
    let raw = raw_get(&base, &alice.token, "/api/users/@me").await;
    check_typed::<bs_types::User>("rest.me", &raw, &mut findings).unwrap();
    let raw = raw_get(&base, &alice.token, &format!("/api/spaces/{space_id}")).await;
    check_typed::<bs_types::SpaceWithChannelsAndMembers>("rest.space", &raw, &mut findings)
        .unwrap();

    // ─── Администрирование (первый пользователь инстанса — админ) ──────────
    if me.is_admin {
        alice.api.instance_settings().await.unwrap();
        let invite = alice
            .api
            .create_invite(&NewInvite {
                name: "контракт".into(),
                max_uses: Some(1),
                expires_at: None,
            })
            .await
            .unwrap();
        alice.api.invites(false).await.unwrap();
        alice.api.revoke_invite(&invite.id).await.unwrap();
        alice.api.storage_stats().await.unwrap();
    }

    // ─── Модерация и уход ──────────────────────────────────────────────────
    alice
        .api
        .ban_member(&space_id, &carol.id, Some("тест"))
        .await
        .unwrap();
    assert!(
        alice
            .api
            .list_bans(&space_id)
            .await
            .unwrap()
            .iter()
            .any(|b| b.user_id == carol.id)
    );
    alice.api.unban_member(&space_id, &carol.id).await.unwrap();
    bob.api.leave_space(&space_id, &bob.id).await.unwrap();
    // Удаление пространства сервер не рассылает событием: проверяем по REST.
    alice.api.delete_space(&space_id).await.unwrap();
    assert!(alice.api.space(&space_id).await.is_err());

    // ─── Отчёт ─────────────────────────────────────────────────────────────
    for socket in [&alice_ws, &bob_ws, &carol_ws] {
        findings.merge(&socket.findings());
    }
    let report = render(&findings);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/contract-report.txt");
    let _ = std::fs::write(&path, &report);
    eprintln!("{report}");
    assert!(
        findings.parse_errors.is_empty(),
        "кадры не разобрались:\n{report}"
    );
    assert!(
        findings.unknown_events.is_empty(),
        "неизвестные события:\n{report}"
    );
    if std::env::var("BS_CONTRACT_STRICT").is_ok_and(|v| v == "1") {
        assert!(
            findings.dropped_fields.is_empty(),
            "потерянные поля:\n{report}"
        );
    }
}

fn render(findings: &Findings) -> String {
    let mut out = String::from("Контрактный отчёт backspace-rs\n\n");
    section(&mut out, "Ошибки разбора", findings.parse_errors.iter());
    section(
        &mut out,
        "Неизвестные события",
        findings.unknown_events.iter(),
    );
    let dropped: BTreeSet<&String> = findings.dropped_fields.iter().collect();
    section(
        &mut out,
        "Поля, которые клиент не хранит",
        dropped.into_iter(),
    );
    out
}

fn section<'a>(out: &mut String, title: &str, items: impl Iterator<Item = &'a String>) {
    let items: Vec<&String> = items.collect();
    let _ = writeln!(out, "## {title}: {}", items.len());
    for item in items {
        let _ = writeln!(out, "  - {item}");
    }
    out.push('\n');
}
