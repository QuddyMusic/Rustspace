# backspace-rs

*English · [Русский](README.ru.md)*

A cross-platform client for [Backspace](https://github.com/TheZwiss/backspace), written entirely
in Rust. The main client is graphical (`egui`/`eframe`, crate `bs-app`, binary `backspace-rs`);
a terminal client, `bs-tui`, sits next to it as a protocol testbed.

The goal is full compatibility with the original: the client talks to unmodified Backspace
instances (including federated ones) and supports everything the original web and desktop
clients do. What is still missing is listed in the [roadmap](ROADMAP.md).

## Status

The whole workspace builds; `cargo clippy --workspace --all-targets --all-features -D warnings`
is clean and every test passes. Compatibility is checked against the real server: the
contract stand (`cargo xtask contract`) starts the original Node server and strictly parses
every WebSocket frame and REST response the client receives.

### What the GUI does

- **Sign-in.** Login and registration (with invite code and live username check). Plain `http`
  outside loopback only with explicit consent. The session survives restarts: the token lives
  in the system keyring (Secret Service, Keychain, Credential Manager), with a `0600` file as a
  fallback.
- **Spaces.** Channels and categories, roles and permissions, per-channel overrides, members,
  bans, invites, join requests, the instance directory, notification settings per space and
  per channel (including timed mute).
- **Messages.** Send, edit, delete, replies, reactions, mentions, markdown, read state,
  history paging, message search with filters and jump-to-result. Text longer than the
  server limit (4000 characters) is sent as several messages in a row; a code block cut at
  the break is closed with ` ``` ` and reopened with the same fence (and language) in the
  next message.
- **Files and media.** Uploads with progress, image viewer, downloading attachments (one or
  all), avatars, banners, stickers, GIFs (Klipy search, favourites).
- **Direct messages.** One-to-one and group DMs, friends and requests, user profiles, space
  invites sent as DM cards.
- **Federation.** Several instances at once, remote ones signed in through the home account.
  An invite or a directory entry from an instance that is not connected yet connects it on
  the spot and then joins the space. The Explore page shows the shared directory of other
  instances (`/api/directory`) when the home instance has it configured.
- **Voice and video** (`--features voice`). Voice channels and DM calls over LiveKit; mute and
  deafen; input and output volume 0–200 %, per-user volume and local mute, stream volume and
  attenuation while someone speaks; echo cancellation, noise suppression and AGC; microphone
  test; speaking indicators. Camera (`--features camera`) and screen sharing (monitor or
  window, 720/1080/1440p, 15/30/60 fps) with a call stage above the chat; remote streams are
  watched on demand like in the original, and the sharer sees who is watching.
- **Desktop integration** (`--features desktop`). Tray icon with show/hide, mute, deafen and
  quit; closing the window hides it to the tray. Global hotkeys (including push-to-talk with
  key release) on X11, Windows and macOS; inside the window the hotkeys always work.
- **Look and feel.** Six built-in themes, custom accent, editing any colour, corner radius,
  compact mode, JSON theme import/export, UI scale. Interface sounds from the original.
  System notifications by the original's rules (levels, mutes, DND, mentions).
- **Languages.** Russian and English built in, switchable live; any other language can be
  added as a JSON catalog in `<config dir>/locales/<code>.json` (see *Settings → Language*).
- **Administration.** Profile, password change, account deletion; the instance admin panel
  (settings, users, storage, registration, federation peers, streaming limits, telemetry,
  updates).

Security notes and known limitations are in [AUDIT.md](AUDIT.md) (Russian).

## Workspace

| Crate | Purpose | Tests |
|---|---|---|
| `bs-types` | Protocol data types (users, spaces, channels, messages, events) | 59 |
| `bs-permissions` | Permission bits, role hierarchy, overrides | 13 |
| `bs-api` | REST client (`/api/*`), Bearer JWT, uploads and downloads, coded errors | 108 |
| `bs-ws` | WebSocket client: heartbeat, reconnect, event parsing | 11 |
| `bs-instances` | Connection to one instance: dispatcher, reconnect, password and token sign-in | 14 |
| `bs-identity` | Login and registration, token storage, per-remote credentials, `UserKey` | 31 |
| `bs-media` | Image and GIF decoding with size and pixel limits | 13 |
| `bs-voice` | Voice and video over LiveKit: own audio pipeline (cpal), mixer, video stage, camera, screen capture, data-channel signals | 75 |
| `bs-core` | Instance state and cache, event application, notification rules | 72 |
| `bs-app` | The GUI and its entry point | 215 |
| `bs-tui` | Terminal client (`ratatui`), one instance | 35 |
| `bs-contract` | Contract tests against a real Backspace server | 2 |
| `xtask` | `cargo xtask contract`: starts the original server and runs the contract tests | — |

## Building and running

Rust is pinned in `rust-toolchain.toml`; `rustup` fetches it automatically. Features (voice,
camera, tray…) are chosen **at build time**; the program itself has no feature switches.

### Build

```sh
cargo build --release -p bs-app                                     # GUI without voice
cargo build --release -p bs-app --features voice,camera,desktop     # everything
cargo build --release -p bs-tui                                     # terminal client
```

The package `bs-app` has a single binary called **`backspace-rs`**, `bs-tui` has **`bs-tui`**.
A release build ends up in `target/release/` (a debug build — without `--release` — in
`target/debug/`).

### Run what you built

```sh
./target/release/backspace-rs            # Linux and macOS
.\target\release\backspace-rs.exe        # Windows
./target/release/bs-tui                  # terminal client
./target/release/bs-tui --allow-insecure # bs-tui: allow http outside loopback
```

The binary is self-contained: copy it anywhere and run it. `backspace-rs` takes no
command-line options; everything is set in the app (*Settings*). Only the environment is
read: `RUST_LOG` for logging, e.g.
`RUST_LOG=bs_app=debug,bs_voice=debug ./target/release/backspace-rs`. The login form asks
for the instance address, username and password; for an `http://` instance outside loopback
tick "Allow insecure connection (http)".

To put it on your `PATH` instead:

```sh
cargo install --path crates/bs-app --features voice,camera,desktop   # → ~/.cargo/bin/backspace-rs
backspace-rs
```

### Build and run in one step (for development)

`cargo run` builds the package and starts its binary (`backspace-rs` for `bs-app`), with the
same features you pass:

```sh
cargo run -p bs-app                                    # debug build, no voice
cargo run --release -p bs-app --features voice         # optimised, with voice
cargo run -p bs-tui -- --allow-insecure                # arguments after `--` go to the program
```

### Cargo features of `bs-app`

| Feature | What it adds | Extra build requirements |
|---|---|---|
| `sounds` | interface sounds (rodio) | Linux: `libasound2-dev` |
| `voice` | voice, screen sharing, call stage (includes `sounds`) | clang **21+**, `libclang`, `pkg-config`; Linux: PipeWire, X11/Wayland headers; `libwebrtc` is downloaded at build time |
| `camera` | webcam via nokhwa (V4L2, Media Foundation, AVFoundation; includes `voice`) | — |
| `tray` | tray icon and hide-to-tray | Linux: `libgtk-3-dev`, `libxdo-dev`, `libayatana-appindicator3-dev` |
| `global-hotkeys` | hotkeys outside the window (not on pure Wayland) | Linux: X11 headers |
| `desktop` | `tray` + `global-hotkeys` | as above |

Other runtime notes: the keyring needs `libdbus-1-dev` at build time and a Secret Service
daemon at run time (otherwise the token goes to a `0600` file); file dialogs on Linux use
`xdg-desktop-portal` or `zenity`. Audio uses the default ALSA host, which reaches PulseAudio
or PipeWire through their ALSA plugins. `cargo run -p bs-voice --example audio_check` lists
audio devices and shows the microphone level.

### Where data lives

`~/.config/backspace-rs/` on Linux (honours `XDG_CONFIG_HOME`),
`~/Library/Application Support/backspace-rs/` on macOS, `%APPDATA%\backspace-rs\` on Windows:
`session.json` (instance address; the token is in the keyring), `prefs.json` (theme, scale,
voice, sounds, notifications, hotkeys, language), `locales/` (your own translations).

### Packaging

```sh
cargo install cargo-packager --locked
cargo packager --release -p bs-app --formats deb,appimage   # Linux
cargo packager --release -p bs-app --formats dmg            # macOS
cargo packager --release -p bs-app --formats nsis           # Windows
```

Packages land in `target/packages`; the icon and `.desktop` file are in `packaging/linux`.

### Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo xtask contract --server ../backspace          # start the original and check the client
cargo xtask contract --url http://127.0.0.1:3000    # against a running server
```

The contract stand needs Node.js 22+ and `pnpm install` in the Backspace checkout. Aliases:
`cargo fc` (fmt check), `cargo c` (clippy), `cargo t` (tests), `cargo f` (fmt).

## Translations

Strings are keyed by their Russian source text (gettext style). The English catalog is
`crates/bs-app/locales/en.json`; a test fails if any string in the code has no English
translation, and placeholders (`{name}`) must match. To add a language, copy `en.json`, set
`"@language"` to the language's own name, translate the values and drop the file into
`<config dir>/locales/<code>.json` — it shows up in *Settings → Language* immediately.

## Rules

- `unsafe_code = "deny"` across the workspace (`forbid` in `bs-voice`).
- No `.unwrap()`, `.expect()`, `panic!()` or `todo!()` in production code (clippy deny).
- Errors: `thiserror` in libraries, `anyhow` in the application.
- Permissions are `bitflags` over `u64`, serialised as decimal strings (JSON loses bits above
  2^53). Snowflake IDs are strings. `Origin` is an enum `{Home, Remote(Url)}`.

## License

AGPL-3.0-only, like the original Backspace.

Third-party assets: the [Inter](https://rsms.me/inter/) font (`crates/bs-app/assets/fonts/`,
SIL Open Font License 1.1); interface sounds from the original Backspace
(`crates/bs-app/assets/sounds`, AGPL-3.0-only).
