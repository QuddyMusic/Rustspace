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
  history paging, message search with filters and jump-to-result. Code blocks are
  syntax-highlighted by the fence language (` ```bash `, ` ```rust `…; Prism aliases too). Text longer than the
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
  test; speaking indicators. Camera (`--features camera`) and screen sharing with a call stage
  above the chat: libwebrtc capture (the ScreenCast portal on Wayland), 540p–4K or native,
  30–120 fps, VP9 or H.264, smoothness or clarity, all fitted to the instance's streaming
  limits; system audio without Backspace's own sound (per-application capture over
  PulseAudio/PipeWire on Linux, output loopback on Windows and macOS). Remote streams and
  their audio are received only while you watch them, and the sharer sees who is watching.
- **Desktop integration** (`--features desktop`). Tray icon with show/hide, mute, deafen and
  quit; closing the window hides it to the tray. Global hotkeys (including push-to-talk with
  key release) on X11, Windows and macOS, and on Wayland through the XDG GlobalShortcuts
  portal (shortcuts are assigned in the system settings); inside the window the hotkeys
  always work.
- **Look and feel.** Six built-in themes, custom accent, editing any colour, corner radius
  (the whole window is made of rounded modules that follow it), compact mode, JSON theme
  import/export, UI scale. Interface sounds from the original.
  System notifications by the original's rules (levels, mutes, DND, mentions).
- **Languages.** Russian and English built in, switchable live; any other language can be
  added as a JSON catalog in `<config dir>/locales/<code>.json` (see *Settings → Language*).
- **"Backspace" page** in the sidebar, like the original's project hub: what's new on your
  home instance (with an "updated" dot), project links, bug reports, this server's version and
  source.
- **Client updates.** *Settings → About the client* checks the releases of the client's own
  GitHub repository (`owner/repo`; a default can be baked in with `BS_UPDATE_REPO=owner/repo`
  at build time) and offers the download for your system.
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
| `bs-voice` | Voice and video over LiveKit: own audio pipeline (cpal), mixer, video stage, camera, screen capture, system audio, data-channel signals | 68 |
| `bs-core` | Instance state and cache, event application, notification rules | 72 |
| `bs-app` | The GUI and its entry point | 232 |
| `bs-tui` | Terminal client (`ratatui`), one instance | 35 |
| `bs-contract` | Contract tests against a real Backspace server | 2 |
| `bs-server` | The Rust server (in progress): the original's REST API and WebSocket gateway over the same database, see "Server" | 31 |
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
| `tray` | tray icon and hide-to-tray | Linux: `libgtk-3-dev`, `libayatana-appindicator3-dev` |
| `global-hotkeys` | hotkeys outside the window (X11, Windows, macOS; Wayland through the GlobalShortcuts portal) | Linux: X11 headers |
| `desktop` | `tray` + `global-hotkeys` | as above |

Other runtime notes: the keyring needs `libdbus-1-dev` at build time and a Secret Service
daemon at run time (otherwise the token goes to a `0600` file); file dialogs on Linux use
`xdg-desktop-portal` or `zenity`; screen sharing on Wayland needs `xdg-desktop-portal` with a
ScreenCast backend (GNOME, KDE or wlr) and PipeWire, system audio on Linux needs PulseAudio or
`pipewire-pulse`. Audio uses the default ALSA host, which reaches PulseAudio
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

## Server (`bs-server`)

The Rust server mirrors the original Backspace server: the same routes, WebSocket events,
environment variables and the same SQLite database. The original's database file opens as is
(same migrations; applied ones are not repeated), and the other way round: a database that
`bs-server` has worked with opens in the original. Tokens and password hashes are shared.

**Ported:** sign-up and sign-in, invites; profile, password, account deletion; spaces,
members, roles, bans, channels, categories, permission overrides; messages, reactions, read
positions, pokes; direct and group conversations; friends and people search; notification
settings; voice channels, calls, voice moderation, LiveKit tokens; instance settings. Every
route and event is compared with the original by the scenarios in `tools/compat`.

**Not there yet:** file uploads (tus) and serving `/api/uploads`, link previews, message
search, the space directory and join requests, stickers, GIFs, storage and users in the admin
panel, serving the web client, federation (delivering and receiving events between
instances). Next on the plan: PostgreSQL, two-factor authentication and end-to-end encrypted
DMs; see [ROADMAP.md](ROADMAP.md). The server is not yet fit for an instance with federation
peers, and says so at startup.

### Build

```sh
cargo build --release -p bs-server      # target/release/backspace-server
```

Only a C compiler is needed (SQLite is built in). No Node.js.

### Run

```sh
JWT_SECRET="$(openssl rand -hex 32)" \
DB_PATH=data/backspace.db UPLOAD_DIR=data/uploads \
target/release/backspace-server
```

The server listens on `0.0.0.0:3000`. The first registered user becomes the administrator.
It stops on `Ctrl+C` or `SIGTERM`, letting open requests finish.

To move over from the original server: stop it, copy the database and start
`backspace-server` with the same `JWT_SECRET`, `DB_PATH` and `UPLOAD_DIR`. Going back works
the same way.

### Configuration

The environment variables are the original's.

| Variable | Default | Meaning |
|---|---|---|
| `JWT_SECRET` | none (required) | Token signing key, at least 32 characters |
| `JWT_EXPIRES_IN` | `30d` | Token lifetime (`30d`, `12h`, `3600`) |
| `PORT`, `HOST` | `3000`, `0.0.0.0` | Where to listen |
| `DB_PATH` | `data/backspace.db` | SQLite database file |
| `UPLOAD_DIR` | `data/uploads` | Directory for uploaded files |
| `MAX_UPLOAD_SIZE` | `104857600` | File size limit, bytes |
| `DOMAIN` | none | Instance domain (invite links, `user@domain` names) |
| `PUBLIC_ORIGIN` | none | Full public address when it is not `https://DOMAIN` |
| `TRUSTED_PROXY_HOPS` | `1` | How many proxies in front write `X-Forwarded-For` (0–4). Rate limits key on it: `1` behind one nginx, `0` with no proxy |
| `REGISTRATION_OPEN` | `true` | Whether sign-up works without an invite (the admin can change it in settings) |
| `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` | none | LiveKit server for voice; voice is off without them |
| `DIRECTORY_ENDPOINT` | the project's directory | Space directory address; an empty string turns it off |
| `DISABLE_RATE_LIMITS` | `false` | Turn rate limits off (tests only) |
| `BACKSPACE_SOURCE_URL`, `BACKSPACE_COMMIT` | none | Shown in the instance info |
| `BS_PASSWORD_HASH_COST` | `12` | bcrypt cost (4–16); change for tests only |
| `RUST_LOG` | `info` | Log detail (`debug`, `bs_server=trace`) |

Put a TLS reverse proxy in front that passes WebSocket through on `/ws`.

### Compatibility check

`tools/compat` starts the original server and `bs-server` on copies of one database and runs
the same scenarios against both, comparing responses and WebSocket events:

```sh
tools/compat/up.sh                    # needs a Backspace checkout with `pnpm install`
python3 tools/compat/s_spaces.py      # also s_channels, s_messages, s_dm, s_social,
                                      # s_account, s_voice, s_admin; cmp_ready.py
```

The checkout path comes from the `ORIG` variable (see the top of `up.sh`).

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
(`crates/bs-app/assets/sounds`, AGPL-3.0-only); syntax grammars and colour themes from
[bat](https://github.com/sharkdp/bat) via `two-face` (each under its own licence, see that crate).
