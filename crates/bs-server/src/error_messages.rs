//! Коды ошибок и их английские тексты — как `ERROR_MESSAGES` оригинала.
//!
//! Файл сгенерирован из `utils/httpErrors.ts`; клиенты показывают текст по
//! коду, но поле `error` должно совпадать с оригиналом.

/// Текст ошибки по коду (`{{имя}}` — место для значения из `details`).
#[must_use]
pub(crate) fn message(code: &str) -> Option<&'static str> {
    MESSAGES
        .binary_search_by(|(key, _)| (*key).cmp(code))
        .ok()
        .map(|index| MESSAGES[index].1)
}

/// Пары «код — текст», отсортированные по коду.
static MESSAGES: &[(&str, &str)] = &[
    (
        "PEER_EXISTS_RESET_REQUIRED",
        "A peer record for this instance already exists; reset it first",
    ),
    (
        "PEER_HANDSHAKE_IN_PROGRESS",
        "This instance is completing its own peering handshake with you; yours will not be used",
    ),
    (
        "PEER_REVOKED",
        "Peering with this instance has been revoked",
    ),
    (
        "accent_color_invalid",
        "Accent color must be a valid hex color (e.g. #ff0000)",
    ),
    ("account_deleted", "This account has been deleted"),
    (
        "account_deletion_blocked_owned_spaces",
        "Transfer or delete the spaces you own before deleting your account",
    ),
    (
        "account_deletion_password_incorrect",
        "Password is incorrect",
    ),
    (
        "admin_cannot_delete_self",
        "Use account settings to delete your own account",
    ),
    ("already_banned", "User is already banned"),
    ("already_friends", "You are already friends with this user"),
    ("already_member", "You are already a member"),
    ("already_owner", "Cannot transfer to current owner"),
    ("attachment_invalid", "Invalid or already-used attachment"),
    ("attachment_not_found", "Attachment not found"),
    ("attachment_not_owned", "You do not own this attachment"),
    ("avatar_color_invalid", "Invalid avatar color"),
    (
        "avatar_url_invalid",
        "Avatar URL must be a relative upload path or http/https URL",
    ),
    ("ban_not_found", "Ban not found"),
    (
        "banner_url_invalid",
        "Banner URL must be a relative upload path or http/https URL",
    ),
    ("bio_too_long", "Bio can be at most {{max}} characters"),
    (
        "cannot_change_own_roles",
        "You cannot change your own roles",
    ),
    (
        "cannot_change_unowned_permissions",
        "Cannot change permissions you do not possess",
    ),
    (
        "cannot_deny_unowned_permissions",
        "Cannot deny permissions you do not possess",
    ),
    ("cannot_dm_self", "Cannot create DM with yourself"),
    ("cannot_friend_self", "You cannot add yourself as a friend"),
    (
        "cannot_grant_unowned_permissions",
        "Cannot grant permissions you do not possess",
    ),
    ("cannot_invite_self", "Cannot invite yourself"),
    (
        "cannot_target_owner",
        "The space owner cannot be removed or banned",
    ),
    ("cannot_target_self", "You cannot do that to yourself"),
    (
        "category_name_length",
        "Category name must be between {{min}} and {{max}} characters",
    ),
    ("category_name_required", "Category name is required"),
    ("category_not_found", "Category not found"),
    (
        "category_not_in_space",
        "Category {{id}} does not belong to this space",
    ),
    (
        "channel_name_length",
        "Channel name must be between {{min}} and {{max}} characters",
    ),
    ("channel_name_required", "Channel name is required"),
    ("channel_not_found", "Channel not found"),
    (
        "channel_not_in_space",
        "Channel {{id}} does not belong to this space",
    ),
    ("channel_topic_invalid", "Channel topic must be a string"),
    (
        "channel_topic_length",
        "Channel topic can be at most {{max}} characters",
    ),
    (
        "channel_type_invalid",
        "Channel type must be \"text\" or \"voice\"",
    ),
    ("cleanup_age_invalid", "{{field}} must be a positive number"),
    (
        "content_required",
        "Message must have content or attachments",
    ),
    (
        "content_too_long",
        "Message content must be {{max}} characters or less",
    ),
    (
        "current_password_incorrect",
        "Current password is incorrect",
    ),
    (
        "custom_status_too_long",
        "Custom status can be at most {{max}} characters",
    ),
    (
        "deleted_user_password_reset",
        "Cannot reset password of a deleted user",
    ),
    (
        "deleted_user_role_change",
        "Cannot change role of a deleted user",
    ),
    (
        "directory_disabled",
        "The directory is not configured on this instance",
    ),
    (
        "directory_private_space",
        "A private space cannot be listed in the directory",
    ),
    (
        "directory_requires_discovery",
        "Turn on space discovery before enabling the directory",
    ),
    (
        "directory_unreachable",
        "The directory could not be reached",
    ),
    (
        "display_name_too_long",
        "Display name can be at most {{max}} characters",
    ),
    (
        "dm_call_in_progress",
        "A call is already running in this conversation",
    ),
    ("dm_call_not_found", "This call has already ended"),
    ("dm_not_found", "DM channel not found"),
    (
        "dm_not_group",
        "This is a 1-on-1 DM; that action needs a group DM",
    ),
    ("dm_owner_only", "Only the group owner can do that"),
    (
        "dm_target_required",
        "userId or (homeUserId + homeInstance) is required",
    ),
    (
        "duplicate_users",
        "Duplicate users after identity resolution",
    ),
    (
        "everyone_role_not_assignable",
        "@everyone role is implicit and cannot be assigned",
    ),
    (
        "everyone_role_not_deletable",
        "Cannot delete the @everyone role",
    ),
    (
        "federated_registration_closed",
        "Federated registration is closed on this instance",
    ),
    (
        "federated_user_cannot_be_admin",
        "Federated users cannot be promoted to admin",
    ),
    (
        "federated_user_password_managed_by_home",
        "Federated users authenticate via their home instance",
    ),
    (
        "federation_credential_failed",
        "Failed to issue federation credential",
    ),
    (
        "federation_credential_home_only",
        "Federation credentials are issued by your home instance",
    ),
    (
        "federation_credential_remote_only",
        "Federation credentials are only issued for remote instances",
    ),
    (
        "federation_different_password",
        "The account on that instance does not accept the credential your home instance issued",
    ),
    ("field_not_boolean", "{{field}} must be a boolean"),
    ("file_not_found", "File not found"),
    ("forbidden", "Forbidden"),
    ("friend_request_not_found", "Friend request not found"),
    (
        "friend_request_not_pending",
        "Can only cancel pending requests",
    ),
    (
        "friend_request_not_recipient",
        "You can only manage requests sent to you",
    ),
    (
        "friend_request_not_sender",
        "You can only cancel requests you sent",
    ),
    (
        "friend_request_pending",
        "A friend request is already pending",
    ),
    (
        "group_dm_includes_self",
        "Do not include yourself; you are added automatically",
    ),
    (
        "group_dm_name_length",
        "Group DM name must be between {{min}} and {{max}} characters",
    ),
    (
        "group_dm_too_few_members",
        "users must contain at least {{min}} entries",
    ),
    (
        "group_dm_too_many_members",
        "Group DMs are limited to {{max}} members",
    ),
    ("home_instance_invalid", "Invalid homeInstance domain"),
    ("icon_not_image", "Icon must be an image"),
    ("icon_too_large", "Icon must be smaller than {{max}} MB"),
    ("icon_url_invalid", "Invalid icon URL"),
    (
        "incoming_request_exists",
        "This user has already sent you a friend request",
    ),
    (
        "instance_name_length",
        "Instance name must be {{min}}-{{max}} characters",
    ),
    (
        "instance_settings_missing",
        "Instance settings not initialized",
    ),
    (
        "instance_settings_reload_failed",
        "Failed to read updated settings",
    ),
    ("internal_error", "Something went wrong on the server"),
    ("invalid_body", "Invalid request body"),
    ("invalid_credentials", "Invalid username or password"),
    ("invalid_target", "Invalid target"),
    ("invalid_target_domain", "Invalid target domain"),
    ("invite_code_required", "Invite code is required"),
    ("invite_invalid", "Invite is invalid or expired"),
    ("invite_not_found", "Invalid invite code"),
    ("invite_required", "An invite is required to register"),
    (
        "join_request_decided",
        "This request has already been decided",
    ),
    ("join_request_not_found", "Join request not found"),
    (
        "join_request_pending",
        "You already have a pending request for this space",
    ),
    (
        "join_request_required",
        "This space takes join requests; send one and a manager will review it",
    ),
    (
        "last_admin_cannot_be_demoted",
        "Cannot demote the last admin",
    ),
    (
        "layout_arrays_required",
        "channels and categories arrays are required",
    ),
    (
        "lookup_failed",
        "Looking up the user on the remote instance failed",
    ),
    (
        "lookup_rate_limited",
        "Too many lookups; try again in a minute",
    ),
    ("media_cleanup_failed", "Media cleanup failed: {{reason}}"),
    (
        "member_nickname_invalid",
        "Nicknames can be at most {{max}} characters on one line, with at least one visible character and no control characters.",
    ),
    ("member_not_found", "Member not found"),
    ("member_update_failed", "Failed to update member"),
    ("message_create_failed", "Failed to create message"),
    ("message_delete_forbidden", "You cannot delete this message"),
    (
        "message_edit_not_author",
        "You can only edit your own messages",
    ),
    ("message_empty", "Message must have content or attachments"),
    (
        "message_lookup_failed",
        "Failed to load the message after saving it",
    ),
    ("message_not_found", "Message not found"),
    ("message_update_failed", "Failed to update message"),
    ("missing_permission", "Missing {{permission}} permission"),
    (
        "native_account_required",
        "Only native accounts can mint attach proofs",
    ),
    (
        "new_owner_not_member",
        "New owner must be a member of the space",
    ),
    (
        "new_owner_required",
        "newOwnerId or (homeUserId + homeInstance) is required",
    ),
    ("no_fields_to_update", "No fields to update"),
    ("not_a_friend", "You can only add friends to group DMs"),
    (
        "not_authoritative_for_sender",
        "This instance is not authoritative for the sender",
    ),
    ("not_dm_member", "You are not a member of this DM channel"),
    ("not_found", "Not found"),
    ("not_friends", "You are not friends with this user"),
    (
        "not_message_author",
        "You can only edit or delete your own messages",
    ),
    ("not_space_member", "You are not a member of this space"),
    ("origin_invalid", "Invalid origin"),
    ("origin_required", "Origin is required"),
    (
        "override_bits_invalid",
        "allow and deny must be valid decimal integer strings",
    ),
    (
        "override_target_invalid",
        "targetType must be \"role\" or \"member\"",
    ),
    ("override_target_required", "targetId is required"),
    (
        "overrides_conflict",
        "Someone else changed this override since it was loaded. Load it again and review it before saving.",
    ),
    (
        "owner_cannot_kick_self",
        "Owners cannot kick themselves; use leave instead",
    ),
    ("ownership_transfer_failed", "Failed to transfer ownership"),
    (
        "paging_cursor_conflict",
        "Use either before or after, not both",
    ),
    ("password_required", "Password is required"),
    (
        "password_too_long",
        "Password can be at most {{max}} characters",
    ),
    (
        "password_too_short",
        "Password must be at least {{min}} characters",
    ),
    (
        "password_unchanged",
        "New password must differ from the current password",
    ),
    (
        "peer_pending",
        "Federation with the remote instance is still being established",
    ),
    (
        "peer_pending_approval",
        "The remote instance has not approved federation yet",
    ),
    (
        "peer_pending_local_admin",
        "Your admin has not approved federation with this instance yet",
    ),
    (
        "peer_rejected",
        "The remote instance has rejected federation",
    ),
    (
        "peer_reset_pending",
        "Federation with this instance is being reset; try again shortly",
    ),
    ("peer_unreachable", "The remote instance is unreachable"),
    ("permissions_invalid", "Invalid permissions value"),
    (
        "poke_target_no_access",
        "That member cannot see this channel",
    ),
    ("position_invalid", "Position must be a non-negative number"),
    (
        "profile_managed_by_home",
        "Profile fields are managed by your home instance",
    ),
    ("rate_limited", "Too many requests"),
    (
        "reattach_handle_taken",
        "Another account on this instance signs in with that handle",
    ),
    ("recipient_deleted", "This user's account was deleted"),
    ("registration_closed", "Registration is closed"),
    (
        "registry_conflict",
        "Conflict: incoming registry is not newer than stored version",
    ),
    (
        "relay_ttl_out_of_range",
        "federationRelayTtlDays must be an integer between {{min}} and {{max}}",
    ),
    (
        "replicated_username_format_required",
        "Replicated users must use username@domain format",
    ),
    ("reply_target_invalid", "Invalid reply target"),
    (
        "role_hierarchy",
        "You can only do that to members and roles ranked below your highest role",
    ),
    ("role_ids_invalid", "roleIds must be an array of role IDs"),
    ("role_name_required", "Role name cannot be empty"),
    ("role_name_taken", "A role with this name already exists"),
    (
        "role_not_in_space",
        "Role {{roleId}} does not belong to this space",
    ),
    (
        "role_permissions_conflict",
        "Someone else changed this role's permissions since they were loaded. Load them again and review them before saving.",
    ),
    (
        "rotation_interval_out_of_range",
        "defaultAutoRotateIntervalDays must be an integer between {{min}} and {{max}}",
    ),
    ("space_create_failed", "Failed to create space"),
    ("space_load_failed", "Failed to load space"),
    (
        "space_name_length",
        "Space name must be between {{min}} and {{max}} characters",
    ),
    ("space_name_required", "Space name is required"),
    ("space_not_found", "Space not found"),
    ("space_not_public", "This space does not allow public joins"),
    (
        "space_not_requestable",
        "This space does not accept join requests",
    ),
    (
        "space_owner_cannot_leave",
        "Space owner cannot leave. Transfer ownership or delete the space.",
    ),
    ("space_owner_only", "Only the space owner can do that"),
    (
        "space_owner_title_invalid",
        "Owner titles can be 1 to {{max}} characters on one line, with at least one visible character and no control characters.",
    ),
    (
        "space_requires_approval",
        "This space requires approval to join",
    ),
    ("space_update_failed", "Failed to update space"),
    (
        "space_uses_join_requests",
        "Request-only spaces do not use invite links; entry is by join request",
    ),
    (
        "space_visibility_invalid",
        "Visibility must be \"public\", \"request\", or \"private\"",
    ),
    ("status_invalid", "Invalid status"),
    (
        "sticker_collection_full",
        "A sticker collection holds at most {{max}} stickers",
    ),
    (
        "sticker_invalid_image",
        "Upload a PNG, JPEG, WebP or GIF image of at most {{max}} MB",
    ),
    (
        "sticker_name_invalid",
        "Sticker names can be 1 to {{max}} characters on one line, with at least one visible character and no control characters.",
    ),
    ("sticker_not_found", "Sticker not found"),
    (
        "sticker_not_local",
        "Only stickers from this instance can be added to a collection",
    ),
    ("storage_cleanup_failed", "Cleanup failed: {{reason}}"),
    (
        "storage_orphans_failed",
        "Failed to list orphaned files: {{reason}}",
    ),
    (
        "storage_stats_failed",
        "Failed to compute storage stats: {{reason}}",
    ),
    (
        "streaming_bitrate_matrix_invalid",
        "bitrateMatrixOverrides must be an object or null",
    ),
    (
        "streaming_bitrate_matrix_key_invalid",
        "Invalid matrix key: \"{{key}}\". Keys must be {resolution}_{framerate}, e.g. \"1080_60\"",
    ),
    (
        "streaming_bitrate_matrix_value_invalid",
        "Invalid value for \"{{key}}\": must be a positive number up to {{max}} kbps",
    ),
    (
        "streaming_bitrate_step_out_of_range",
        "bitrateStepKbps must be between {{min}} and {{max}}",
    ),
    (
        "streaming_framerates_invalid",
        "Invalid framerates: {{invalid}}. Allowed: {{allowed}}",
    ),
    (
        "streaming_framerates_required",
        "allowedFramerates must be a non-empty array",
    ),
    (
        "streaming_max_bitrate_out_of_range",
        "maxBitrateKbps must be between {{min}} and {{max}}",
    ),
    (
        "streaming_max_framerate_invalid",
        "maxFramerate must be one of: {{allowed}}",
    ),
    (
        "streaming_max_resolution_invalid",
        "maxResolution must be one of: {{allowed}}",
    ),
    (
        "streaming_min_bitrate_not_below_max",
        "minBitrateKbps must be less than maxBitrateKbps",
    ),
    (
        "streaming_min_bitrate_out_of_range",
        "minBitrateKbps must be between {{min}} and {{max}}",
    ),
    (
        "streaming_resolutions_invalid",
        "Invalid resolutions: {{invalid}}. Allowed: {{allowed}}",
    ),
    (
        "streaming_resolutions_required",
        "allowedResolutions must be a non-empty array",
    ),
    (
        "system_message_immutable",
        "System messages cannot be edited",
    ),
    (
        "target_domain_required",
        "targetDomain is required (string)",
    ),
    (
        "target_not_dm_member",
        "Target user is not a member of this DM channel",
    ),
    ("unauthorized", "Unauthorized"),
    (
        "upload_limit_out_of_range",
        "maxUploadSizeMb must be a positive integer ({{min}} - {{max}})",
    ),
    (
        "upload_session_cleanup_failed",
        "Tus cleanup failed: {{reason}}",
    ),
    ("user_already_deleted", "User is already deleted"),
    ("user_banned", "You are banned from this space"),
    ("user_create_failed", "Failed to create user"),
    ("user_id_required", "userId is required"),
    ("user_not_found", "User not found"),
    (
        "user_owns_spaces",
        "User owns spaces. Transfer ownership first",
    ),
    (
        "username_characters_invalid",
        "Username can only contain lowercase letters, numbers, and underscores",
    ),
    ("username_confirmation_mismatch", "Username does not match"),
    (
        "username_confirmation_required",
        "Username confirmation is required",
    ),
    (
        "username_domain_part_invalid",
        "Username domain part is invalid",
    ),
    (
        "username_invalid",
        "Username must be {{min}}-{{max}} lowercase alphanumeric/underscore characters",
    ),
    (
        "username_length_invalid",
        "Username must be between {{min}} and {{max}} characters",
    ),
    (
        "username_local_part_invalid",
        "Username local part must be {{min}}-{{max}} lowercase alphanumeric/underscore characters",
    ),
    ("username_required", "Username is required"),
    ("username_taken", "Username is already taken"),
    (
        "username_too_long",
        "Usernames can be at most {{max}} characters",
    ),
    ("users_not_found", "One or more users not found"),
    ("validation_failed", "Validation failed"),
    ("voice_connect_forbidden", "Missing CONNECT permission"),
    (
        "voice_disabled",
        "Voice/video is not configured on this server",
    ),
];
