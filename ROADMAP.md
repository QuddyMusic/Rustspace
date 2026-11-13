# backspace-rs roadmap

*English · [Русский](ROADMAP.ru.md)*

A cross-platform Backspace client written entirely in Rust, and later a Rust server. Both must
stay fully compatible with existing Backspace instances and support everything the original
does. Compatibility is checked by the contract stand (`cargo xtask contract`), which runs the
original server.

## Done

### Foundation and protocol
- Workspace, CI (fmt, clippy `-D warnings` with all features, tests, `cargo doc`, `cargo audit`,
  contract job against the real server), clang 21 toolchain for `libwebrtc`.
- `bs-types`, `bs-permissions`, `bs-api`, `bs-ws`, `bs-instances`, `bs-identity`, `bs-core`,
  `bs-media`: the full protocol, REST and WebSocket, reconnects, permissions, cache.
- Multiple instances at once; federated instances signed in through the home account.

### GUI client
- Sign-in, registration, session restore with the token in the system keyring.
- Spaces, channels, categories, roles, overrides, members, bans, invites, join requests,
  directory, notification settings; DMs and group DMs, friends, profiles.
- Messages: edit, delete, replies, reactions, mentions, markdown, read state, history,
  search with jump-to-result; uploads, downloads, image viewer, stickers, GIFs.
- Account and instance administration.
- Themes (six built in, custom colours, radius, compact mode, import/export), UI scale,
  interface sounds, system notifications, hotkeys, all client settings in `prefs.json`.
- Packaging metadata for deb, AppImage, dmg and nsis.

### Voice, video and desktop
- **Localization.** Every interface string goes through a gettext-style catalog: Russian
  (source) and English built in, switchable live in *Settings → Language*; extra languages
  as JSON files in the config directory. A test keeps the English catalog complete.
- **Own audio pipeline.** `PlatformAudio` in libwebrtc gave neither input gain nor per-track
  volume, and its local track produced no PCM (the microphone test meter stayed silent).
  Audio now runs through cpal: microphone → APM (echo, noise, AGC) → gain → LiveKit, and
  remote tracks → mixer → speakers, with the playback signal fed back for echo cancellation.
  This brings input and output volume 0–200 %, per-user volume and local mute, stream volume
  and attenuation while someone speaks — the same settings as the original. Checked
  end-to-end against a local LiveKit server.
- **LiveKit identities.** The server issues `homeUserId:displayName`; the client compared the
  whole string with user ids, so speaking indicators never lit up. Fixed.
- **Camera and screen sharing.** Call stage above the chat with local previews and remote
  streams; monitor or window picker; webcam through nokhwa
  (`camera` feature). Remote streams are watched on demand and their audio is heard only by
  viewers; the data-channel protocol of the original (`stream_watch`, `stream_stop`,
  `stream_republish`, `stream_resume`, `deafen`) is implemented both ways, so the sharer sees
  who is watching. `voice_status` now carries camera and screen flags (DM calls included),
  and channel members show camera and LIVE badges.
- **Tray and global hotkeys** (`desktop` feature). Tray menu (show, hide, mute, deafen, quit)
  and hide-to-tray on close, like the original desktop app; the client keeps working while
  hidden. Global hotkeys on X11, Windows and macOS, including push-to-talk release.
- **Federated join.** Invites and the join dialog connect an instance that is not connected
  yet through the home account and then join the space. Explore shows the shared directory
  of other instances (`GET /api/directory`) with "connect and join / request".
- **Refactoring.** `chat.rs`, `bridge.rs` and `dialogs.rs` are split into modules.

### This iteration
- **Screen sharing rebuilt.** Capture now uses libwebrtc's own desktop capturer instead of
  xcap: on Wayland it goes through the ScreenCast portal (the system dialog picks the screen
  or window, so the right one is shared), elsewhere monitors and windows are listed directly.
  Frames are converted with libyuv (no per-pixel Rust loops), the preview is downscaled and
  throttled, and a zero-size frame can no longer crash wgpu. Up to 120 fps.
- **Streaming limits enforced.** With the LiveKit token the client fetches the host's
  `/api/settings/streaming` and fits the saved choice to it, like the original's
  `effectiveScreenShareConfig`: allowed resolutions (540p–4K, native) and frame rates, custom
  bitrate allowed or not and its range, the original's bitrate matrix with admin overrides.
  The picker shows only what the host allows, plus smoothness/clarity (degradation
  preference), VP9 or H.264 and the resulting bitrate.
- **System audio in screen sharing.** Published as `ScreenshareAudio` (stereo, 128 kbps).
  On Linux every application's stream is recorded separately over the PulseAudio protocol
  (works with pipewire-pulse) and mixed, leaving out Backspace's own playback — viewers do not
  hear the call back, like venmic in the original. Windows and macOS use output loopback.
- **Streams are received only when watched.** LiveKit auto-subscribe is off: voice and
  cameras are subscribed at once, a screen share and its audio only after "Watch" — remote
  1080p60 streams are no longer downloaded for nothing.
- **Wayland global hotkeys** through the XDG GlobalShortcuts portal (press and release, so
  push-to-talk works); shortcuts are assigned in the system settings and shown in the app.
  Covered by a test against a fake portal on a private D-Bus.
- **"Backspace" page** in the sidebar (the original's project hub): what's new with an
  "updated" dot per user, support, Insights, bug reports with version and client, hosting
  guide, this server's name, version and source link.
- **Client update checker.** *Settings → About the client*: checks the releases of the
  client's own GitHub repository (`owner/repo`, also settable at build time with
  `BS_UPDATE_REPO`) every six hours or on demand, compares by semver, offers the download for
  this OS and architecture, "what's new" and "skip this version".
- **Corner radius everywhere.** The main window is made of rounded modules (sidebar, chat,
  members, call stage) on a backdrop; every frame, card, dialog, menu and highlight scales
  with the radius setting. At 0 the window looks as before.
- **No `allow(clippy::too_many_lines)` left** anywhere in the workspace: the composer, admin
  panels, Markdown parser, icons, contract tests and TUI are split into smaller functions.

## Remaining client work
- **Not verified on real hardware**: the audio pipeline and system audio were tested with
  virtual PulseAudio/PipeWire devices and a local LiveKit, Wayland capture and the
  GlobalShortcuts portal with a headless compositor and a fake portal; the camera only builds.
  Needs a run on a real machine (GNOME/KDE on Wayland, Windows, macOS).
- The client's GitHub repository for update checks has to be set (in the settings or with
  `BS_UPDATE_REPO`) until the owner names it.
- **TUI** lags behind the GUI (no DM feed, groups, space settings or voice UI). Postponed by
  the owner's decision.

## Server stage (in progress)
1. **Security audit.** Client: done. Server: runs alongside the port: 22 findings (S1–S22),
   19 fixed in `bs-server`, the rest kept for compatibility and described; see
   [SECURITY-AUDIT.md](SECURITY-AUDIT.md). Federation remains: it is audited as it is ported.
2. **Rust server** (`bs-server`, Axum), compatible with existing clients and instances.
   Build, run and configuration: [README.md](README.md#server-bs-server).
   - [x] Configuration, storage layer running the original SQLite migrations (one database
     file opens in both servers, both ways), rate limits, security headers.
   - [x] Registration with invites, login, tokens; profile, password, account deletion.
   - [x] WebSocket gateway: auth, `ready`, heartbeat, presence, activity.
   - [x] Spaces, members, roles, bans, channels, categories, permission overrides.
   - [x] Messages, reactions, read positions, pokes.
   - [x] Direct and group conversations, friends, people search.
   - [x] Voice: rooms, calls, moderation, LiveKit tokens.
   - [x] Notification settings, registration invites, instance settings.
   - [x] Files: uploads (tus), ranged downloads, thumbnails, daily storage cleanup.
   - [x] Link previews (guarded against requests into the internal network), metadata.
   - [x] Message search and "messages around".
   - [x] Space directory, public join and join requests; directory document and feed.
   - [x] Stickers (animated too), GIF.
   - [x] Admin: users, storage, update status, telemetry (the choice is stored, but the
     server sends nothing).
   - [x] Serving the web client (`BS_WEB_DIST`), content policy, CSP reports.
   - [x] Forced voice disconnect through the LiveKit API (finding S16).
   - [x] Compatibility stand (`tools/compat`): 17 scenarios, responses and events compared
     with the original on SQLite and on PostgreSQL.
   - [ ] Federation. In the original this is about 16 thousand lines of code (about 36
     thousand with tests), and the part most sensitive to mistakes; it is ported in steps, each
     compared with the original on a two-instance stand. Until then `bs-server` runs as a
     standalone instance. The mutation log is already written as in the original.
     - [x] Request signatures between instances (HMAC, freshness, secret rotation), checked
       against the original.
     - [x] Handshake and peer management: peering in both directions, approval and denial, user
       requests and their notifications, secret rotation, epochs and reset detection,
       background peer checks, replay protection. Checked on a stand of two instance pairs
       (`tools/compat/fed_up.sh`, `f_peering.py`: original ↔ original against `bs-server` ↔
       original and `bs-server` ↔ `bs-server`, on SQLite and on PostgreSQL), 194 steps with
       no differences. Along the way request body parsing now repeats `JSON.parse` and
       `secure-json-parse`: signatures are computed over the same text, and parse errors
       carry the V8 messages; the content type picks the parser (415), and the body is
       parsed before the auth check, as in Fastify (the `s_bodies` scenario). A peer's sync
       time (`lastSyncedAt`) arrives with the sync step.
     - [x] Outbox and the delivery worker: conversation events (messages, edits, deletions,
       reactions, read positions, closing, groups: members, owner, name), friendship,
       profile and presence events are queued for peers in the same transaction as the
       change, with the mutation log and subject clocks; queue folding, batched delivery to
       `/api/federation/relay`, retries with backoff, unreachable peers and their recovery,
       refusals by the receiver (with the friend request rollback), placeholder peers and
       handshakes for queued mail, event expiry. Checked on the stand with the original as
       the receiver (`f_outbox.py`: delivery and retries, on SQLite and on PostgreSQL). For
       the stand, federation addresses instances by https names through a TLS front, as the
       original does. A friend request to someone on another instance and typing arrive with
       the lookup and calls steps.
     - [x] Inbound events: `/api/federation/relay` (signature, replay protection, 90 batches
       a minute per peer, the batch source bound to the signing peer) and
       `DELETE /api/federation/identity`; messages (with mentions, replies, system space
       invites and attachments queued for download), edits, deletions (with tombstones
       against a late create), reactions, read positions, closing and reopening, groups
       (bootstrap by the first `member_add`, members, owner, name and icon), friendship
       (with the applied-event ledger and subject clocks), profile and presence of replicas.
       Each event is checked for attribution: a peer speaks only for its own users, and for
       ours only when the user holds an account there. Replicas are created and named as in
       the original; profile pictures are downloaded through an SSRF-safe fetch (8 MB, images
       only), attachments by a background queue. Each event is applied in its own transaction;
       when files are needed the transaction is rolled back, the files are downloaded outside
       it and the event is applied again. Checked on the stand (`f_inbound.py`: `inbound` and
       `relay_checks`, original ↔ original against `bs-server` ↔ original and `bs-server` ↔
       `bs-server`, on SQLite and on PostgreSQL), 48 steps with no differences.
     - [x] Account replicas and login from another instance, `user@domain` lookup: the
       `users/lookup`, `users/by-home-id` and `verify-attach-proof` peer routes (60 a minute
       per peer, signed answers for the proof); friend requests to `user@domain` and by
       identity (peering on demand, lookup, a replica with the home's profile, the event);
       asking a new replica's home for its record and a pass over incomplete replicas when a
       peer becomes active (names, the profile with its version); signing in to a replica with
       the home password (the hash is refreshed after the home and its epoch confirm it); the
       attach proof, re-attaching a detached account (merging its replica, reconciling
       one-to-one conversation keys), credentials for other instances and deleting one's
       replicas on them. Checked on the stand (`f_lookup.py`: `friends` and `accounts`, plus
       `f_inbound.py` and `f_outbox.py` now without allowances for the missing lookup), on
       SQLite and PostgreSQL, with `bs-server` on one and on both sides, with no differences.
     - [ ] Sync after downtime and state reconciliation.
     - [ ] Cross-instance calls.
     - [ ] Two-instance stand scenarios for delivery, inbound events, replicas and calls;
       the audit. The stand itself exists (`tools/compat/fed_up.sh`).
   - [x] The `bs-contract` stand against `bs-server`: both tests pass (REST: 42 calls; the
     client scenario: no parse errors, no unknown events).
3. **PostgreSQL** as a storage option: done. A backend behind the same interface
   (`DATABASE_URL`), the schema is derived from the original migrations and extends itself,
   row order in responses matches SQLite, data import: `backspace-server import-sqlite <file>`.
   Left: TLS to the database (for now: trusted network or same host only).
4. **Two-factor authentication** (TOTP and recovery codes): done, server (`/api/auth/2fa`,
   code at login) and client (field at login, card in account settings). Old clients and the
   original server are unaffected.
5. **End-to-end encryption for DMs only**: first version done.
   - [x] The scheme and its implementation (`bs-e2ee`): X25519, HKDF-SHA256,
     XChaCha20-Poly1305. An encrypted message travels as plain text `bs-e2ee:1:…`, so it
     passes through any server, including the original and federated ones; the original uses
     the `encryption_version` column for its federation envelope version, so it is not
     usable for this.
   - [x] Public key directory in `bs-server` (`/api/e2ee`); encrypted messages are excluded
     from search.
   - [x] Client: the account key is kept on the device (a `0600` file next to the saved
     session), encryption is turned on per conversation from its header, keys travel as a
     message in the conversation itself and through the directory, there is a safety number;
     when the peer's key changes, sending stops until it is accepted.
   - [ ] A check with two real clients through the original server and through federation
     (the logic is covered by tests without a network for now).
   - [ ] Moving the key to a second device (each device has its own key for now, and the
     peer sees that as a key change); keeping the key in the system secret store.
   - Out of the first version: group conversations, attachments, forward secrecy, search
     over encrypted messages.

## Technical notes
- Permissions (`bs-permissions`) only drive what the UI shows; the server always decides.
- `unsafe` is denied workspace-wide (`forbid` in `bs-voice`).
