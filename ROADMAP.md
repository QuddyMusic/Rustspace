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
