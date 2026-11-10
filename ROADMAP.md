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

### This iteration
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
  streams; monitor or window picker, 720/1080/1440p at 15/30/60 fps; webcam through nokhwa
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
- **Refactoring.** `chat.rs`, `bridge.rs` and `dialogs.rs` are split into modules; long
  functions are broken up and those modules no longer need `allow(clippy::too_many_lines)`.

## Remaining client work
- **Not verified on real hardware**: the new audio pipeline was tested with virtual
  PulseAudio devices and a local LiveKit, the camera only builds (no webcam on the
  development machine). Needs a run on a real machine with a microphone, speakers and camera.
- Global hotkeys under pure Wayland need the XDG GlobalShortcuts portal (the original uses
  it); today they fall back to in-window hotkeys there.
- System audio in screen sharing (the original desktop captures it on some platforms).
- Streaming limits set by the instance admin are not yet enforced on the sender side.
- The "Backspace" project hub entry in the sidebar; the update checker of the desktop app.
- `allow(clippy::too_many_lines)` remains in a few UI files outside the refactored modules
  (composer, admin panels, markdown, icons).
- **TUI** lags behind the GUI (no DM feed, groups, space settings or voice UI). Postponed by
  the owner's decision.

## Next stages (after the owner's go-ahead)
1. **Security audit** of client and server code.
2. **Rust server** (Axum), compatible with existing clients and instances and checked by the
   same contract stand: core → everything else → federation.
3. **PostgreSQL** as a storage option for the server.
4. **Two-factor authentication.**
5. **End-to-end encryption for DMs only**, without breaking compatibility with instances that
   do not support it (so that an instance admin can no longer read private messages).

## Technical notes
- Permissions (`bs-permissions`) only drive what the UI shows; the server always decides.
- `unsafe` is denied workspace-wide (`forbid` in `bs-voice`).
