//! Тесты применения событий.

use bs_types::{
    MessageWithUser, PresenceIdentity, ServerEvent, SpaceVoiceState, UserStatus, VoiceStateAction,
};

use super::{apply, apply_at};
use crate::state::{Cache, TYPING_TTL_MS};

const READY_JSON: &str = r#"{
    "user": {
        "id": "u1", "username": "alice", "displayName": null, "avatar": null,
        "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
        "status": "online", "customStatus": null, "isAdmin": false,
        "createdAt": 1, "homeInstance": null, "homeUserId": null,
        "replicatedInstances": []
    },
    "spaces": [{
        "id": "s1", "name": "Space", "ownerId": "u1",
        "visibility": "public", "directoryListed": false, "createdAt": 1,
        "channels": [{
            "id": "c1", "spaceId": "s1", "name": "general", "type": "text",
            "position": 0, "createdAt": 1
        }],
        "categories": [], "members": [], "roles": []
    }],
    "dmChannels": [{
        "id": "dm1", "federatedId": null, "ownerId": null,
        "ownerHomeUserId": null, "ownerHomeInstance": null, "createdAt": 1,
        "members": [], "metadataUpdatedAt": 2
    }],
    "supportsPoke": true,
    "voiceStates": {},
    "unreadCounts": {"c1": 2},
    "pendingApprovalCount": 0
}"#;

const ALICE: &str = r#"{
    "id":"u1","username":"alice","displayName":null,"avatar":null,
    "banner":null,"accentColor":null,"avatarColor":null,"bio":null,
    "status":"online","customStatus":null,"isAdmin":false,
    "createdAt":1,"homeInstance":null,"homeUserId":null,
    "replicatedInstances":[]
}"#;

fn cache() -> Cache {
    let payload = serde_json::from_str(READY_JSON).expect("ready парсится");
    Cache::from_ready(payload)
}

fn message_json(id: &str, created_at: i64, content: &str) -> String {
    format!(
        r#"{{"id":"{id}","channelId":"c1","userId":"u1","replyToId":null,
            "content":"{content}","editedAt":null,"createdAt":{created_at},
            "user":{ALICE},"attachments":[],"embeds":[],"reactions":[],"replyTo":null}}"#
    )
}

fn message(id: &str, created_at: i64, content: &str) -> MessageWithUser {
    serde_json::from_str(&message_json(id, created_at, content)).expect("сообщение парсится")
}

fn event(json: &str) -> ServerEvent {
    serde_json::from_str(json).expect("событие парсится")
}

#[test]
fn message_created_inserts_sorted_and_updates_channel() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        ServerEvent::MessageCreated {
            message: message("m2", 20, "b")
        }
    ));
    assert!(apply(
        &mut c,
        ServerEvent::MessageCreated {
            message: message("m1", 10, "a")
        }
    ));
    let ids: Vec<_> = c
        .messages("c1")
        .iter()
        .map(|m| m.message.id.as_str())
        .collect();
    assert_eq!(ids, ["m1", "m2"]);
    assert_eq!(
        c.channel("c1").and_then(|ch| ch.last_message_id.as_deref()),
        Some("m1")
    );
}

#[test]
fn message_updated_replaces_not_duplicates() {
    let mut c = cache();
    let _ = apply(
        &mut c,
        ServerEvent::MessageCreated {
            message: message("m1", 10, "a"),
        },
    );
    let _ = apply(
        &mut c,
        ServerEvent::MessageUpdated {
            message: message("m1", 10, "edited"),
        },
    );
    assert_eq!(c.messages("c1").len(), 1);
    assert_eq!(
        c.messages("c1")[0].message.content.as_deref(),
        Some("edited")
    );
}

#[test]
fn message_deleted_returns_false_for_unknown() {
    let mut c = cache();
    assert!(!apply(
        &mut c,
        ServerEvent::MessageDeleted {
            message_id: "nope".to_string(),
            channel_id: "c1".to_string(),
        }
    ));
    let _ = apply(
        &mut c,
        ServerEvent::MessageCreated {
            message: message("m1", 10, "a"),
        },
    );
    assert!(apply(
        &mut c,
        ServerEvent::MessageDeleted {
            message_id: "m1".to_string(),
            channel_id: "c1".to_string(),
        }
    ));
    assert!(c.messages("c1").is_empty());
}

#[test]
fn reaction_added_and_removed_are_idempotent() {
    let mut c = cache();
    let _ = apply(
        &mut c,
        ServerEvent::MessageCreated {
            message: message("m1", 10, "a"),
        },
    );
    let added = event(
        r#"{"type":"reaction_added","messageId":"m1",
            "reaction":{"id":"r1","messageId":"m1","userId":"u2",
            "emoji":"👍","createdAt":1}}"#,
    );
    assert!(apply(&mut c, added.clone()));
    assert!(!apply(&mut c, added));
    assert_eq!(c.messages("c1")[0].reactions.len(), 1);

    let removed =
        event(r#"{"type":"reaction_removed","messageId":"m1","userId":"u2","emoji":"👍"}"#);
    assert!(apply(&mut c, removed.clone()));
    assert!(!apply(&mut c, removed));
    assert!(c.messages("c1")[0].reactions.is_empty());
}

#[test]
fn presence_update_changes_status_and_activities() {
    let mut c = cache();
    let ev = event(
        r#"{"type":"presence_update","userId":"u1","status":"dnd",
            "activities":[{"type":"playing","name":"Rust"}],
            "homeUserId":"h1","homeInstance":"remote.example"}"#,
    );
    assert!(apply(&mut c, ev));
    assert_eq!(c.me().status, UserStatus::DoNotDisturb);
    assert_eq!(c.user_activities("u1").len(), 1);
    assert_eq!(
        c.user_activity_identity("u1"),
        Some(&PresenceIdentity {
            home_user_id: Some("h1".to_string()),
            home_instance: Some("remote.example".to_string()),
        })
    );

    let clear =
        event(r#"{"type":"presence_update","userId":"u1","status":"online","activities":[]}"#);
    assert!(apply(&mut c, clear));
    assert!(c.user_activities("u1").is_empty());
}

#[test]
fn channel_created_and_deleted() {
    let mut c = cache();
    let created = event(
        r#"{"type":"channel_created","spaceId":"s1",
            "channel":{"id":"c2","spaceId":"s1","name":"random",
            "type":"text","position":1,"createdAt":2}}"#,
    );
    assert!(apply(&mut c, created));
    assert_eq!(c.channels("s1").count(), 2);

    let deleted = event(r#"{"type":"channel_deleted","spaceId":"s1","channelId":"c2"}"#);
    assert!(apply(&mut c, deleted));
    assert_eq!(c.channels("s1").count(), 1);
    assert!(!apply(
        &mut c,
        event(r#"{"type":"channel_deleted","spaceId":"s1","channelId":"c2"}"#)
    ));
}

#[test]
fn voice_join_leave_and_moved() {
    let mut c = cache();
    let join = event(
        r#"{"type":"voice_state_update","channelId":"c1","userId":"u2",
            "action":"join","channelElapsedSeconds":5}"#,
    );
    assert!(apply(&mut c, join));
    assert_eq!(c.users_in_voice("c1"), ["u2".to_string()]);
    assert_eq!(c.voice_channel_elapsed_seconds("c1"), Some(5));

    let moved = event(
        r#"{"type":"voice_moved","userId":"u2",
            "oldChannelId":"c1","newChannelId":"c2"}"#,
    );
    assert!(apply(&mut c, moved));
    assert!(c.users_in_voice("c1").is_empty());
    assert_eq!(c.users_in_voice("c2"), ["u2".to_string()]);

    let leave =
        event(r#"{"type":"voice_state_update","channelId":"c2","userId":"u2","action":"leave"}"#);
    assert!(apply(&mut c, leave));
    assert!(c.users_in_voice("c2").is_empty());
}

#[test]
fn space_mute_applies_only_to_me() {
    let mut c = cache();
    let other = event(
        r#"{"type":"voice_space_muted","userId":"u2","channelId":"c1",
            "spaceId":"s1","muted":true}"#,
    );
    assert!(!apply(&mut c, other));
    assert!(c.space_voice_state("s1").is_none());

    let me = event(
        r#"{"type":"voice_space_muted","userId":"u1","channelId":"c1",
            "spaceId":"s1","muted":true}"#,
    );
    assert!(apply(&mut c, me));
    assert_eq!(
        c.space_voice_state("s1"),
        Some(&SpaceVoiceState {
            space_muted: true,
            space_deafened: false,
            permission_muted: None,
        })
    );
}

#[test]
fn ack_and_mark_unread() {
    let mut c = cache();
    assert_eq!(c.unread_count("c1"), 2);
    assert!(apply(
        &mut c,
        event(r#"{"type":"channel_ack","channelId":"c1","messageId":"m9"}"#)
    ));
    assert_eq!(c.unread_count("c1"), 0);
    assert_eq!(
        c.read_state("c1")
            .and_then(|s| s.last_read_message_id.as_deref()),
        Some("m9")
    );

    assert!(apply(
        &mut c,
        event(r#"{"type":"mark_unread","channelId":"c1","messageId":"m8"}"#)
    ));
    assert_eq!(c.unread_count("c1"), 1);
    assert!(!apply(
        &mut c,
        event(r#"{"type":"mark_unread","channelId":"c1","messageId":"m8"}"#)
    ));
}

#[test]
fn federation_tracking_is_deduplicated() {
    let mut c = cache();
    let rejected = event(
        r#"{"type":"federation_peer_rejected","peerOrigin":"evil.example",
            "reason":"nope","affectedContexts":[]}"#,
    );
    assert!(apply(&mut c, rejected.clone()));
    assert!(!apply(&mut c, rejected));
    assert_eq!(c.rejected_peer_origins(), ["evil.example".to_string()]);

    let approval =
        event(r#"{"type":"federation_approval_request_received","origin":"new.example"}"#);
    assert!(apply(&mut c, approval));
    assert_eq!(c.pending_approval_count(), 1);
    assert_eq!(
        c.awaiting_approval_peer_origins(),
        ["new.example".to_string()]
    );
}

#[test]
fn refetch_events_do_not_touch_cache() {
    let mut c = cache();
    assert!(!apply(
        &mut c,
        event(r#"{"type":"space_access_changed","spaceId":"s1"}"#)
    ));
    assert!(!apply(&mut c, event(r#"{"type":"pong"}"#)));
    assert!(!apply(&mut c, event(r#"{"type":"totally_unknown_event"}"#)));
}

#[test]
fn voice_leave_is_idempotent() {
    let mut c = cache();
    let ev =
        event(r#"{"type":"voice_state_update","channelId":"c1","userId":"u1","action":"join"}"#);
    assert!(apply(&mut c, ev));
    let leave =
        event(r#"{"type":"voice_state_update","channelId":"c1","userId":"u1","action":"leave"}"#);
    assert!(apply(&mut c, leave.clone()));
    assert!(!apply(&mut c, leave));
    let _ = VoiceStateAction::Join;
}

// ─── «Печатает…» ──────────────────────────────────────────────────────────

const TYPING_U2: &str = r#"{"type":"typing","channelId":"c1","userId":"u2","username":"bob"}"#;

#[test]
fn typing_is_stored_and_expires() {
    let mut c = cache();
    assert!(apply_at(&mut c, event(TYPING_U2), 1_000));

    let entries: Vec<_> = c.typing_in("c1", 1_000).collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].user_id, "u2");
    assert_eq!(entries[0].username, "bob");
    assert_eq!(entries[0].expires_at, 1_000 + TYPING_TTL_MS);

    // Запись жива ровно до expires_at, дальше — протухла.
    assert!(c.is_typing("c1", "u2", 1_000 + TYPING_TTL_MS - 1));
    assert!(!c.is_typing("c1", "u2", 1_000 + TYPING_TTL_MS));
    assert!(c.typing_in("c1", 1_000 + TYPING_TTL_MS).next().is_none());
}

#[test]
fn typing_refresh_does_not_duplicate() {
    let mut c = cache();
    let _ = apply_at(&mut c, event(TYPING_U2), 1_000);
    let _ = apply_at(&mut c, event(TYPING_U2), 3_000);

    let entries: Vec<_> = c.typing_in("c1", 3_000).collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].expires_at, 3_000 + TYPING_TTL_MS);
}

#[test]
fn typing_for_unknown_channel_is_kept() {
    let mut c = cache();
    let ev = event(r#"{"type":"typing","channelId":"ghost","userId":"u2","username":"bob"}"#);
    assert!(apply_at(&mut c, ev, 0));
    assert!(c.is_typing("ghost", "u2", 0));
}

// ─── Fallback-статусы ─────────────────────────────────────────────────────

#[test]
fn presence_update_tracks_status_outside_spaces_and_dms() {
    let mut c = cache();
    assert!(c.user_status("stranger").is_none());

    let ev = event(r#"{"type":"presence_update","userId":"stranger","status":"idle"}"#);
    assert!(apply(&mut c, ev));
    assert_eq!(c.user_status("stranger"), Some(UserStatus::Idle));
}

#[test]
fn repeated_presence_with_same_status_does_not_report_change() {
    let mut c = cache();
    let ev = event(r#"{"type":"presence_update","userId":"stranger","status":"offline"}"#);
    assert!(apply(&mut c, ev.clone()));
    assert!(!apply(&mut c, ev));
    assert_eq!(c.user_status("stranger"), Some(UserStatus::Offline));
}

#[test]
fn user_updated_refreshes_status_fallback() {
    let mut c = cache();
    let ev = event(
        r#"{"type":"user_updated","user":{"id":"u2","username":"bob",
            "displayName":null,"avatar":null,"banner":null,"accentColor":null,
            "avatarColor":null,"bio":null,"status":"dnd","customStatus":null,
            "isAdmin":false,"createdAt":2,"homeInstance":null,"homeUserId":null,
            "replicatedInstances":[]}}"#,
    );
    assert!(apply(&mut c, ev));
    assert_eq!(c.user_status("u2"), Some(UserStatus::DoNotDisturb));
}

#[test]
fn channel_deleted_drops_typing_entries() {
    let mut c = cache();
    let _ = apply_at(&mut c, event(TYPING_U2), 0);
    assert!(c.is_typing("c1", "u2", 0));

    let deleted = event(r#"{"type":"channel_deleted","spaceId":"s1","channelId":"c1"}"#);
    assert!(apply(&mut c, deleted));
    assert!(!c.is_typing("c1", "u2", 0));
}

// ─── Друзья ───────────────────────────────────────────────────────────────

fn friend_json(id: &str, username: &str, added_at: i64) -> String {
    format!(
        r#"{{"id":"{id}","username":"{username}","displayName":null,
            "avatar":null,"banner":null,"accentColor":null,"avatarColor":null,
            "bio":null,"status":"online","customStatus":null,
            "createdAt":1,"addedAt":{added_at},"homeUserId":null,"homeInstance":null}}"#
    )
}

fn friend_request_json(id: &str, from: &str, to: &str) -> String {
    format!(
        r#"{{"id":"{id}","fromId":"{from}","toId":"{to}",
            "status":"pending","createdAt":5,"user":null}}"#
    )
}

fn friend(id: &str, username: &str, added_at: i64) -> bs_types::Friend {
    serde_json::from_str(&friend_json(id, username, added_at)).expect("друг парсится")
}

fn friend_request(id: &str, from: &str, to: &str) -> bs_types::FriendRequest {
    serde_json::from_str(&friend_request_json(id, from, to)).expect("заявка парсится")
}

#[test]
fn friend_requests_are_stored_and_deduplicated() {
    let mut c = cache();
    let incoming = friend_request("r1", "u2", "u1");
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestReceived {
            request: incoming.clone()
        }
    ));
    assert!(!apply(
        &mut c,
        ServerEvent::FriendRequestReceived { request: incoming }
    ));

    let outgoing = friend_request("r2", "u1", "u3");
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestSent { request: outgoing }
    ));

    assert_eq!(c.friend_requests().count(), 2);
    assert_eq!(c.incoming_friend_requests().count(), 1);
    assert_eq!(c.outgoing_friend_requests().count(), 1);
    assert_eq!(
        c.incoming_friend_requests().next().map(|r| r.id.as_str()),
        Some("r1")
    );
    assert!(c.friend_request("r2").is_some());
    assert!(c.friend_request("nope").is_none());
}

#[test]
fn friend_request_accepted_adds_friend_and_drops_request() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestReceived {
            request: friend_request("r1", "u2", "u1")
        }
    ));

    let bob = friend("u2", "bob", 10);
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestAccepted {
            friend: bob.clone(),
            request_id: "r1".to_owned()
        }
    ));
    assert!(c.friend_request("r1").is_none());
    assert_eq!(c.friend("u2"), Some(&bob));
    assert_eq!(c.friends().count(), 1);
    assert_eq!(c.user_status("u2"), Some(UserStatus::Online));

    // Повторное принятие той же заявки ничего не меняет.
    assert!(!apply(
        &mut c,
        ServerEvent::FriendRequestAccepted {
            friend: bob,
            request_id: "r1".to_owned()
        }
    ));
}

#[test]
fn friend_request_cancelled_and_declined_remove_request() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestSent {
            request: friend_request("r1", "u1", "u2")
        }
    ));
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestReceived {
            request: friend_request("r2", "u3", "u1")
        }
    ));

    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestCancelled {
            request_id: "r1".to_owned(),
            user_id: "u2".to_owned()
        }
    ));
    assert!(c.friend_request("r1").is_none());

    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestDeclined {
            request_id: "r2".to_owned(),
            user_id: "u1".to_owned()
        }
    ));
    assert!(c.friend_request("r2").is_none());

    // Повторное удаление — без изменений.
    assert!(!apply(
        &mut c,
        ServerEvent::FriendRequestCancelled {
            request_id: "r1".to_owned(),
            user_id: "u2".to_owned()
        }
    ));
    assert!(c.friend_requests().next().is_none());
}

#[test]
fn friend_removed_drops_friend_only_once() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        ServerEvent::FriendRequestAccepted {
            friend: friend("u2", "bob", 10),
            request_id: "r1".to_owned()
        }
    ));
    assert!(c.friend("u2").is_some());

    assert!(apply(
        &mut c,
        ServerEvent::FriendRemoved {
            user_id: "u2".to_owned()
        }
    ));
    assert!(c.friend("u2").is_none());
    assert!(!apply(
        &mut c,
        ServerEvent::FriendRemoved {
            user_id: "u2".to_owned()
        }
    ));
}

#[test]
fn friends_are_sorted_case_insensitively() {
    let mut c = cache();
    for (id, name) in [("u2", "Zoe"), ("u3", "alice"), ("u4", "Bob")] {
        assert!(apply(
            &mut c,
            ServerEvent::FriendRequestAccepted {
                friend: friend(id, name, 1),
                request_id: format!("r-{id}")
            }
        ));
    }
    let names: Vec<&str> = c.friends().map(|f| f.username.as_str()).collect();
    assert_eq!(names, ["alice", "Bob", "Zoe"]);
}

#[test]
fn friend_relay_failure_does_not_change_cache() {
    let mut c = cache();
    assert!(!apply(
        &mut c,
        ServerEvent::FriendRequestRelayFailed {
            request_id: "r1".to_owned(),
            reason: "timeout".to_owned(),
            message: "узел недоступен".to_owned(),
            target_handle: "bob@remote.example".to_owned()
        }
    ));
    assert!(c.friend_requests().next().is_none());
}

fn dm_message_json(id: &str, created_at: i64, content: &str) -> String {
    format!(
        r#"{{"id":"{id}","dmChannelId":"dm1","userId":"u1","content":"{content}",
            "createdAt":{created_at},"user":{ALICE},"attachments":[],"embeds":[],
            "reactions":[]}}"#
    )
}

fn dm_message(id: &str, created_at: i64, content: &str) -> bs_types::DmMessageWithUser {
    serde_json::from_str(&dm_message_json(id, created_at, content)).expect("dm-сообщение парсится")
}

fn dm_event(kind: &str, id: &str, created_at: i64, content: &str) -> ServerEvent {
    event(&format!(
        r#"{{"type":"{kind}","message":{}}}"#,
        dm_message_json(id, created_at, content)
    ))
}

#[test]
fn dm_message_created_is_stored_and_sets_preview() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m2", 20, "второе")
    ));
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m1", 10, "первое")
    ));

    let ids: Vec<_> = c
        .dm_messages("dm1")
        .iter()
        .map(|m| m.message.id.as_str())
        .collect();
    assert_eq!(ids, ["m1", "m2"], "порядок по времени");
    assert!(
        c.dm_channel("dm1")
            .is_some_and(|dm| dm.last_message.is_some())
    );
}

#[test]
fn dm_message_created_for_unknown_dm_is_still_stored() {
    let mut c = cache();
    c.dm_channels.clear();
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m1", 10, "привет")
    ));
    assert_eq!(c.dm_messages("dm1").len(), 1);
}

#[test]
fn dm_message_updated_replaces_content() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m1", 10, "до")
    ));
    assert!(apply(
        &mut c,
        dm_event("dm_message_updated", "m1", 10, "после")
    ));
    assert_eq!(c.dm_messages("dm1").len(), 1);
    assert_eq!(
        c.dm_messages("dm1")[0].message.content.as_deref(),
        Some("после")
    );
}

#[test]
fn dm_message_deleted_removes_message() {
    let mut c = cache();
    assert!(apply(&mut c, dm_event("dm_message_created", "m1", 10, "x")));
    let deleted = event(r#"{"type":"dm_message_deleted","messageId":"m1","dmChannelId":"dm1"}"#);
    assert!(apply(&mut c, deleted.clone()));
    assert!(c.dm_messages("dm1").is_empty());
    assert!(
        !apply(&mut c, deleted),
        "повторное удаление ничего не меняет"
    );
}

#[test]
fn dm_embeds_resolved_updates_message() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m1", 10, "ссылка")
    ));
    let resolved =
        event(r#"{"type":"dm_embeds_resolved","messageId":"m1","dmChannelId":"dm1","embeds":[]}"#);
    assert!(apply(&mut c, resolved));
}

#[test]
fn reactions_apply_to_dm_messages() {
    let mut c = cache();
    assert!(apply(&mut c, dm_event("dm_message_created", "m1", 10, "x")));
    let added = event(&format!(
        r#"{{"type":"reaction_added","messageId":"m1","reaction":{{
            "id":"r1","messageId":"m1","userId":"u1","emoji":"👍","createdAt":1,
            "user":{ALICE}}}}}"#
    ));
    assert!(apply(&mut c, added));
    assert_eq!(c.dm_messages("dm1")[0].reactions.len(), 1);
    let removed =
        event(r#"{"type":"reaction_removed","messageId":"m1","userId":"u1","emoji":"👍"}"#);
    assert!(apply(&mut c, removed));
    assert!(c.dm_messages("dm1")[0].reactions.is_empty());
}

#[test]
fn merge_history_orders_dedups_and_keeps_live_version() {
    let mut c = cache();
    assert!(apply(
        &mut c,
        dm_event("dm_message_created", "m3", 30, "живое")
    ));
    let added = c.merge_dm_history(
        "dm1",
        vec![
            dm_message("m1", 10, "старое"),
            dm_message("m2", 20, "среднее"),
            dm_message("m3", 30, "из истории"),
        ],
    );
    assert_eq!(added, 2, "m3 уже есть");
    let ids: Vec<_> = c
        .dm_messages("dm1")
        .iter()
        .map(|m| m.message.id.as_str())
        .collect();
    assert_eq!(ids, ["m1", "m2", "m3"]);
    assert_eq!(
        c.dm_messages("dm1")[2].message.content.as_deref(),
        Some("живое")
    );
}

#[test]
fn merge_history_for_space_channel_is_idempotent() {
    let mut c = cache();
    let page = || vec![message("a", 10, "1"), message("b", 20, "2")];
    assert_eq!(c.merge_history("c1", page()), 2);
    assert_eq!(c.merge_history("c1", page()), 0);
    assert_eq!(c.messages("c1").len(), 2);
}

#[test]
fn equal_timestamps_are_ordered_by_snowflake_id() {
    let mut c = cache();
    // «9» короче «10», значит и меньше как snowflake.
    c.merge_history("c1", vec![message("10", 5, "b"), message("9", 5, "a")]);
    let ids: Vec<_> = c
        .messages("c1")
        .iter()
        .map(|m| m.message.id.as_str())
        .collect();
    assert_eq!(ids, ["9", "10"]);
}

#[test]
fn ready_clears_dm_messages() {
    let mut c = cache();
    assert!(apply(&mut c, dm_event("dm_message_created", "m1", 10, "x")));
    let payload = serde_json::from_str(READY_JSON).expect("ready парсится");
    assert!(apply(&mut c, ServerEvent::Ready(Box::new(payload))));
    assert!(c.dm_messages("dm1").is_empty());
}
