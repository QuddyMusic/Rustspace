//! Широкий обход REST API против настоящего сервера.
//!
//! В отличие от `scenario.rs`, здесь шаги не останавливают тест при первой
//! ошибке: каждый неудачный вызов записывается, и в конце печатается полный
//! список. Так один прогон показывает все расхождения сразу.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Debug;

use bs_api::{
    ApiClient, ChangePasswordRequest, ChannelUpdate, CreateDmRequest, CreateSpaceRequest,
    FriendRequestDecision, GroupMember, GroupUpdate, InstanceSettingsPatch, InvitePatch,
    MessageSearchQuery, NewChannel, NewInvite, NewRole, OverrideScope, PermissionOverride,
    RegisterRequest, RoleUpdate, SendFriendRequest, SendMessageBody, StreamingPatch,
};
use bs_contract::{server_url, unique};
use bs_types::{OverrideTargetType, SpaceVisibility, UpdateNotificationSettingRequest};

/// PNG 16×16 (красный квадрат).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0xf3, 0xff,
    0x61, 0x00, 0x00, 0x00, 0x19, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x9f, 0x12, 0xcc, 0x30, 0x6a, 0xc0, 0xa8, 0x01, 0xa3, 0x06, 0x0c, 0x17, 0x03, 0x00, 0x30, 0xc4,
    0xfe, 0x10, 0x1c, 0x27, 0xe4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

#[derive(Default)]
struct Log {
    failures: Vec<String>,
    passed: usize,
}

impl Log {
    fn check<T, E: Debug>(&mut self, label: &str, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(value) => {
                self.passed += 1;
                Some(value)
            }
            Err(err) => {
                self.failures.push(format!("{label}: {err:?}"));
                None
            }
        }
    }
}

struct Account {
    api: ApiClient,
    id: String,
    username: String,
    password: String,
}

async fn register(base: &str, prefix: &str) -> Account {
    let mut api = ApiClient::home(base).unwrap();
    let username = unique(prefix);
    let password = "coverage-password-1".to_owned();
    let auth = api
        .register(&RegisterRequest {
            username: username.clone(),
            password: password.clone(),
            ..RegisterRequest::default()
        })
        .await
        .unwrap();
    api.set_token(Some(auth.token));
    Account {
        api,
        id: auth.user.id,
        username,
        password,
    }
}

/// Участники сценария.
struct Cast {
    owner: Account,
    guest: Account,
    third: Account,
}

/// Пространство, созданное для проверки.
struct SpaceCtx {
    id: String,
    text: String,
}

async fn friends(log: &mut Log, cast: &Cast) {
    let Cast {
        owner,
        guest,
        third,
    } = cast;
    // Друзья: нужны для групповых бесед.
    for other in [&guest, &third] {
        if let Some(id) = log.check(
            "send_friend_request",
            owner
                .api
                .send_friend_request(&SendFriendRequest {
                    username: Some(other.username.clone()),
                    ..SendFriendRequest::default()
                })
                .await,
        ) {
            log.check(
                "respond_friend_request",
                other
                    .api
                    .respond_friend_request(&id, FriendRequestDecision::Accept)
                    .await,
            );
        }
    }
    log.check("friend_requests", owner.api.friend_requests().await);
    log.check(
        "search_users",
        owner.api.search_users(&guest.username).await,
    );
    log.check(
        "discover_users",
        owner.api.discover_users("", Some(10), None).await,
    );
}

async fn create_space(log: &mut Log, cast: &Cast) -> SpaceCtx {
    let Cast { owner, guest, .. } = cast;
    // ─── Пространство, каналы, категории, права ────────────────────────────
    let space = owner
        .api
        .create_space(&CreateSpaceRequest {
            name: unique("cov"),
            visibility: Some(SpaceVisibility::Public),
            description: Some("покрытие".into()),
            avatar_color: Some("teal".into()),
        })
        .await
        .unwrap();
    let full = owner.api.space(&space.id).await.unwrap();
    let text = full
        .channels
        .iter()
        .find(|c| c.channel_type == bs_types::ChannelType::Text)
        .unwrap()
        .id
        .clone();
    let category = full.categories.first().map(|c| c.id.clone());
    log.check("spaces", owner.api.spaces().await);
    log.check("channels", owner.api.channels(&space.id).await);
    log.check("explore_spaces", guest.api.explore_spaces("", 20, 0).await);
    log.check(
        "join_public_space",
        guest.api.join_public_space(&space.id).await,
    );
    log.check(
        "update_channel",
        owner
            .api
            .update_channel(
                &text,
                &ChannelUpdate {
                    topic: Some("новая тема".into()),
                    ..ChannelUpdate::default()
                },
            )
            .await,
    );
    if let Some(category) = &category {
        log.check(
            "rename_category",
            owner.api.rename_category(category, "Переименовано").await,
        );
        let scope = OverrideScope::Category(category.clone());
        log.check(
            "list_overrides(category)",
            owner.api.list_overrides(&scope).await,
        );
    }
    SpaceCtx { id: space.id, text }
}

async fn permissions(log: &mut Log, cast: &Cast, space: &SpaceCtx) {
    let Cast { owner, guest, .. } = cast;
    let text = &space.text;
    let scope = OverrideScope::Channel(text.clone());
    log.check(
        "put_override",
        owner
            .api
            .put_override(
                &scope,
                &PermissionOverride {
                    target_type: OverrideTargetType::Member,
                    target_id: guest.id.clone(),
                    allow: "0".into(),
                    deny: "2048".into(),
                },
            )
            .await,
    );
    log.check("list_overrides", owner.api.list_overrides(&scope).await);
    log.check(
        "delete_override",
        owner
            .api
            .delete_override(&scope, OverrideTargetType::Member, &guest.id)
            .await,
    );
    if let Some(role) = log.check(
        "create_role",
        owner
            .api
            .create_role(
                &space.id,
                &NewRole {
                    name: "Роль".into(),
                    color: "#7dd3fc".into(),
                    permissions: None,
                },
            )
            .await,
    ) {
        log.check(
            "update_role",
            owner
                .api
                .update_role(
                    &space.id,
                    &role.id,
                    &RoleUpdate {
                        name: Some("Роль 2".into()),
                        ..RoleUpdate::default()
                    },
                )
                .await,
        );
    }
}

async fn channel_settings(log: &mut Log, cast: &Cast, space: &SpaceCtx) {
    let Cast { owner, guest, .. } = cast;
    let text = &space.text;
    log.check(
        "create_channel(voice)",
        owner
            .api
            .create_channel(
                &space.id,
                &NewChannel {
                    name: "голос".into(),
                    kind: "voice".into(),
                    topic: None,
                    category_id: None,
                },
            )
            .await,
    );
    log.check(
        "update_channel_notifications",
        guest
            .api
            .update_channel_notifications(
                text,
                // Подавление @everyone/@роли — настройка только пространства:
                // для канала сервер его отклоняет.
                &UpdateNotificationSettingRequest {
                    level: Some(Some(bs_types::NotificationLevel::Mentions)),
                    ..UpdateNotificationSettingRequest::default()
                },
            )
            .await,
    );
}

async fn messages(log: &mut Log, cast: &Cast, space: &SpaceCtx) {
    let Cast { owner, .. } = cast;
    let text = &space.text;
    // ─── Сообщения ─────────────────────────────────────────────────────────
    let attachment = log.check(
        "upload_file",
        owner
            .api
            .upload_file("pixel.png", "image/png", PNG, |_, _| {})
            .await,
    );
    let message = log.check(
        "send_channel_message",
        owner
            .api
            .send_channel_message(
                text,
                &SendMessageBody {
                    content: "с вложением".into(),
                    attachments: attachment.iter().map(|a| a.id.clone()).collect(),
                    reply_to_id: None,
                },
            )
            .await,
    );
    if let Some(message) = &message {
        log.check(
            "messages_around",
            owner
                .api
                .messages_around(text, &message.message.id, Some(20))
                .await,
        );
    }
    log.check(
        "search_channel_messages(has)",
        owner
            .api
            .search_channel_messages(
                text,
                &MessageSearchQuery {
                    has: Some("image".into()),
                    ..MessageSearchQuery::default()
                },
            )
            .await,
    );
}

async fn direct_messages(log: &mut Log, cast: &Cast) {
    let Cast {
        owner,
        guest,
        third,
    } = cast;
    // ─── ЛС и группы ───────────────────────────────────────────────────────
    if let Some(dm) = log.check(
        "open_dm",
        owner
            .api
            .open_dm(&CreateDmRequest {
                user_id: Some(guest.id.clone()),
                ..CreateDmRequest::default()
            })
            .await,
    ) {
        log.check("dm_channels", owner.api.dm_channels().await);
        log.check(
            "search_dm_messages",
            owner
                .api
                .search_dm_messages(
                    &dm.id,
                    &MessageSearchQuery {
                        text: Some("x".into()),
                        ..MessageSearchQuery::default()
                    },
                )
                .await,
        );
    }
    if let Some(group) = log.check(
        "create_group_dm",
        owner
            .api
            .create_group_dm(&[GroupMember::local(&guest.id), GroupMember::local(&third.id)])
            .await,
    ) {
        log.check(
            "remove_group_member",
            owner.api.remove_group_member(&group.id, &third.id).await,
        );
        log.check(
            "add_group_member",
            owner.api.add_group_member(&group.id, &third.id).await,
        );
        if let Some(icon) = log.check(
            "upload_file(group icon)",
            owner
                .api
                .upload_file("group-icon.png", "image/png", PNG, |_, _| {})
                .await,
        ) {
            log.check(
                "update_group_dm(icon)",
                owner
                    .api
                    .update_group_dm(
                        &group.id,
                        &GroupUpdate {
                            name: None,
                            icon: Some(Some(icon.filename)),
                        },
                    )
                    .await,
            );
        }
        log.check(
            "transfer_group_owner",
            owner.api.transfer_group_owner(&group.id, &guest.id).await,
        );
    }
}

async fn profile(log: &mut Log, cast: &Cast) {
    let Cast {
        owner,
        guest,
        third,
    } = cast;
    // ─── Стикеры, профиль, пароль ──────────────────────────────────────────
    if let Some(sticker) = log.check("upload_sticker", owner.api.upload_sticker("pix", PNG).await) {
        log.check(
            "delete_sticker",
            owner.api.delete_sticker(&sticker.id).await,
        );
    }
    log.check(
        "verify_password",
        owner.api.verify_password(&owner.password).await,
    );
    log.check(
        "change_password",
        third
            .api
            .change_password(&ChangePasswordRequest {
                current_password: Some(third.password.clone()),
                new_password: "coverage-password-2".into(),
            })
            .await,
    );
    log.check("federation_registry", owner.api.federation_registry().await);
    log.check(
        "notification_settings",
        guest.api.notification_settings().await,
    );
}

async fn ownership(log: &mut Log, cast: &Cast, space: &SpaceCtx) {
    let Cast { owner, guest, .. } = cast;
    // ─── Владение и заявки ─────────────────────────────────────────────────
    log.check(
        "transfer_ownership",
        owner.api.transfer_ownership(&space.id, &guest.id).await,
    );
    log.check(
        "pending_join_requests",
        guest.api.pending_join_requests(&space.id).await,
    );
}

async fn admin(log: &mut Log, cast: &Cast) {
    let Cast { owner, third, .. } = cast;
    log.check("instance_settings", owner.api.instance_settings().await);
    log.check(
        "update_instance_settings",
        owner
            .api
            .update_instance_settings(&InstanceSettingsPatch {
                instance_name: Some("Контракт".into()),
                ..InstanceSettingsPatch::default()
            })
            .await,
    );
    log.check("admin_users", owner.api.admin_users("", 1, false).await);
    log.check(
        "admin_set_role",
        owner.api.admin_set_role(&third.id, false).await,
    );
    log.check(
        "admin_reset_password",
        owner.api.admin_reset_password(&third.id).await,
    );
    log.check("storage_stats", owner.api.storage_stats().await);
    log.check("storage_cleanup", owner.api.storage_cleanup(true).await);
    log.check("streaming_limits", owner.api.streaming_limits().await);
    log.check(
        "update_streaming",
        owner
            .api
            .update_streaming(&StreamingPatch {
                max_bitrate_kbps: Some(6000),
                ..StreamingPatch::default()
            })
            .await,
    );
    log.check("telemetry", owner.api.telemetry().await);
    log.check("telemetry_preview", owner.api.telemetry_preview().await);
    log.check("update_status", owner.api.update_status(false).await);
    log.check("peers", owner.api.peers().await);
    log.check("approval_requests", owner.api.approval_requests().await);
    log.check(
        "admin_delete_user",
        owner.api.admin_delete_user(&third.id).await,
    );
}

async fn admin_invites(log: &mut Log, cast: &Cast) {
    let Cast { owner, .. } = cast;
    if let Some(invite) = log.check(
        "create_invite",
        owner
            .api
            .create_invite(&NewInvite {
                name: "покрытие".into(),
                max_uses: Some(5),
                expires_at: None,
            })
            .await,
    ) {
        log.check(
            "update_invite",
            owner
                .api
                .update_invite(
                    &invite.id,
                    &InvitePatch {
                        name: Some("покрытие 2".into()),
                        ..InvitePatch::default()
                    },
                )
                .await,
        );
        log.check(
            "invite_redemptions",
            owner.api.invite_redemptions(&invite.id).await,
        );
        log.check("revoke_invite", owner.api.revoke_invite(&invite.id).await);
        log.check(
            "reinstate_invite",
            owner
                .api
                .reinstate_invite(&invite.id, &InvitePatch::default())
                .await,
        );
        log.check("invites(archived)", owner.api.invites(true).await);
        log.check("delete_invite", owner.api.delete_invite(&invite.id).await);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_surface_matches_real_server() {
    let Some(base) = server_url() else {
        eprintln!("BS_CONTRACT_URL не задан — контрактный тест пропущен");
        return;
    };
    let mut log = Log::default();
    let cast = Cast {
        owner: register(&base, "own").await,
        guest: register(&base, "gst").await,
        third: register(&base, "thr").await,
    };
    let me = cast.owner.api.me().await.unwrap();
    friends(&mut log, &cast).await;
    let space = create_space(&mut log, &cast).await;
    permissions(&mut log, &cast, &space).await;
    channel_settings(&mut log, &cast, &space).await;
    messages(&mut log, &cast, &space).await;
    direct_messages(&mut log, &cast).await;
    profile(&mut log, &cast).await;
    ownership(&mut log, &cast, &space).await;
    if me.is_admin {
        admin(&mut log, &cast).await;
        admin_invites(&mut log, &cast).await;
    }
    eprintln!(
        "REST: {} вызовов прошло, {} с ошибкой",
        log.passed,
        log.failures.len()
    );
    for failure in &log.failures {
        eprintln!("  - {failure}");
    }
    assert!(log.failures.is_empty(), "есть расхождения с сервером");
}
