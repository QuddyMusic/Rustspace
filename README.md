<div align="center">

<img src="packaging/brand/rustspace/icon.svg" width="112" alt="Rustspace logo">

# Rustspace

**An unofficial Rust client and server compatible with [Backspace](https://github.com/TheZwiss/backspace)**

[![License: AGPL-3.0-only](https://img.shields.io/badge/license-AGPL--3.0--only-b7410e)](LICENSE)
[![Written in Rust](https://img.shields.io/badge/written%20in-Rust-b7410e?logo=rust)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%C2%B7%20Windows%20%C2%B7%20macOS-555)

*English · [Русский](README.ru.md)*

[Install](#installing-the-client) · [Features](#what-the-gui-does) · [Build](#building-and-running) · [Server](#server-bs-server) ·
[Install on a server](#installing-on-a-server-one-script) · [Roadmap](ROADMAP.md) · [License](#license)

</div>

> [!IMPORTANT]
> **Rustspace is an independent, unofficial project.** It is not affiliated with, endorsed by or
> supported by the authors of Backspace. The **official Backspace repository is
> [TheZwiss/backspace](https://github.com/TheZwiss/backspace)**. "Backspace" and its logo are
> trademarks of Jannis Braun; Rustspace uses the name only to say what it is compatible with.
> Please report Rustspace problems [here](https://github.com/QuddyMusic/Rustspace/issues),
> not to Backspace.

Rustspace is written entirely in Rust. The desktop client is graphical (`egui`/`eframe`, crate
`bs-app`, binary `rustspace`); a terminal client, `bs-tui`, sits next to it as a protocol
testbed. The server, `bs-server` (binary `rustspace-server`), is a port of the original server.

The goal is full compatibility with the original: the client talks to unmodified Backspace
instances (including federated ones) and supports everything the original web and desktop
clients do, and `bs-server` serves the original clients too. What is still missing is listed in
the [roadmap](ROADMAP.md).

## Installing the client

Ready builds are on the [releases page](https://github.com/QuddyMusic/Rustspace/releases):

| System | How |
|---|---|
| Linux, any distribution | `curl -fsSL https://raw.githubusercontent.com/QuddyMusic/Rustspace/main/packaging/linux/install.sh \| sh` — into `~/.local`, no root, with a menu entry |
| Linux, AppImage | `Rustspace-x86_64.AppImage`: make it executable and run |
| Debian / Ubuntu | `sudo apt install ./rustspace-linux-x86_64.deb` |
| Arch | `cd packaging/arch && makepkg -si` (`rustspace-bin`, the release binary) |
| Windows | `rustspace-setup-x86_64.exe` — installs for the current user, no admin rights |
| macOS (Apple Silicon) | `rustspace-macos-aarch64.zip` — unsigned: first launch via *Open* in the context menu |

**Updates from inside.** When a new release is out, a strip at the top offers **Update**: the
client downloads the file for its system, checks it against the release's `.sha256`, installs
it and asks to restart. This works for `install.sh`, AppImage, the archive and the Windows
installer; packages (deb, Arch) are updated by the package manager, and the client says so.

## Status

The whole workspace builds; `cargo clippy --workspace --all-targets --all-features -D warnings`
is clean and every test passes. Compatibility is checked against the real server: the
contract stand (`cargo xtask contract`) starts the original Node server and strictly parses
every WebSocket frame and REST response the client receives.

### What the GUI does

- **Sign-in.** Login and registration (with invite code and live username check). Plain `http`
  outside loopback only with explicit consent. The session survives restarts: the token lives
  in the system keyring (Secret Service, Keychain, Credential Manager), with a `0600` file as a
  fallback. "Switch instance" in settings signs out and opens a clean login form; the login
  form lists recent instances (one click fills the address).
- **Two-factor authentication** (on a server that supports it, that is `bs-server`): a code at
  login; turning on, recovery codes and turning off in account settings.
- **End-to-end encryption for one-to-one conversations.** Turned on from the conversation
  header; text is encrypted on the device (X25519, XChaCha20-Poly1305) and travels as an
  ordinary message, so it works through any server, the original included. Keys are exchanged
  in the conversation itself and verified with a safety number. Attachments and group
  conversations are not encrypted; there is no forward secrecy. The key lives in the system
  keyring and moves to another device as a passphrase-protected record (the encryption
  window). The other person needs this client: the original web client shows ciphertext. While a
  one-to-one conversation is not encrypted, a thin bar above it says so (and whether the other
  person has a key at all); it closes with one click and stays closed for that conversation.
- **Spaces.** Channels and categories, roles and permissions, per-channel overrides, members,
  bans, invites, join requests, the instance directory, notification settings per space and
  per channel (including timed mute).
- **Formatting and cards.** Markdown like the original web client: headings `#`–`###`, lists
  (`-`, `1.`, nested by indent), `---` rules, quotes `>` and `>>>`, `-#` small text, links
  `[text](url)`. The **Aa** button above the composer adds a toolbar (bold, italic, code,
  spoiler, headings, lists, quote, link) and a **card** builder: a block with a colour bar,
  title, text and picture — handy for rules and announcements (needs `bs-server`). Link
  previews and webhook cards are shown under messages.
- **Messages.** Send, edit, delete, replies, reactions, mentions, markdown, read state,
  history paging, message search with filters and jump-to-result. Code blocks are
  syntax-highlighted by the fence language (` ```bash `, ` ```rust `…; Prism aliases too). Text longer than the
  server limit (4000 characters) is sent as several messages in a row; a code block cut at
  the break is closed with ` ``` ` and reopened with the same fence (and language) in the
  next message.
- **Files and media.** Uploads with progress, image viewer, downloading attachments (one or
  all), avatars, banners, stickers, GIFs (Klipy search, favourites). An attachment can be
  hidden under a **spoiler** (the eye on the staged file): the picture is blurred until
  clicked. The mark travels as Discord's `SPOILER_` file-name prefix, so it works on any
  server and through the bridges.
- **Saved Messages, pins and bookmarks** (needs `bs-server`). *Saved Messages* is a chat with
  yourself for notes, links and files; any message can be forwarded there from its menu.
  Pinned messages show in a bar above the chat (click — to the message, then to the next
  one): in a one-to-one conversation for yourself or for both, in channels and group DMs for
  everyone (with "Manage messages"). **Bookmarks** are one list across all chats and
  instances, with search and grouping by date, person or chat, a "with media" filter and
  jumping to the original.
- **Translation**, open projects only. "Translate" in the message menu, or automatic
  translation of other people's messages that are not in your language. By default it runs
  on this computer: *Settings → Translation* installs llama.cpp (MIT) and an open Qwen model
  (Apache-2.0) with one button — CPU or GPU (Vulkan, Metal on macOS) — and the client starts
  and stops `llama-server` itself; no keys, the text never leaves the machine. Your own
  LibreTranslate server or any OpenAI-compatible model server (llama-server, vLLM) work too.
  Code, links and mentions are not translated and come back unchanged.
- **Activity (Rich Presence).** Members show what they play, listen to or watch: under the
  name in the member list and as cards with cover art, timers and track progress in the
  profile. The client finds running games itself — the Backspace game dictionary, Steam,
  Flatpak, GOG, Heroic, Bottles, Lutris, games under Wine and Proton — takes music from any
  MPRIS player on Linux, and runs a local Rich Presence server (`discord-ipc-N`), so games such
  as osu!lazer report their map, mode and time. Games can be renamed, hidden, reordered or
  added by hand; their icons come from this computer and are uploaded to the home instance
  (`bs-server`). *Settings → Activity*. Ported from Backspace PR #482 by QuddyMusic and
  cirzear, entirely in Rust.
- **18+ channels** (`bs-server`). A channel marked 18+ in its settings asks every member to
  confirm their age once before showing its messages.
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
  On `bs-server` the microphone uses the instance's voice bitrate (96 kbit/s by default); the
  number next to the call in the voice panel lowers it for a voice channel or a DM call when
  the connection can't keep up.
- **Desktop integration** (`--features desktop`). Tray icon with show/hide, mute, deafen and
  quit; closing the window hides it to the tray. Global hotkeys (including push-to-talk with
  key release) on X11, Windows and macOS, and on Wayland through the XDG GlobalShortcuts
  portal (shortcuts are assigned in the system settings); inside the window the hotkeys
  always work.
- **Look and feel.** Eight built-in themes; the default, **Oxide**, and **Oxide Light** are
  *glass* themes: translucent panels over a soft two-colour glow, glass composer, menus and
  dialogs. Glass is part of the theme — panel opacity, glow strength and glow colours (with
  ready-made pairs) are set in *Appearance* and travel with the theme's JSON file. Also: custom
  accent, editing any colour, corner radius (the whole window is made of rounded modules that
  follow it), compact mode, JSON theme import/export, UI scale, soft or *crisp* text.
  Interface sounds from the original.
  System notifications by the original's rules (levels, mutes, DND, mentions).
- **Languages.** Russian and English built in, switchable live; any other language can be
  added as a JSON catalog in `<config dir>/locales/<code>.json` (see *Settings → Language*).
- **"Backspace" page** in the sidebar, like the original's project hub: what's new on your
  home instance (with an "updated" dot), project links, bug reports, this server's version and
  source.
- **Client updates.** *Settings → About the client* checks the releases of the client's own
  GitHub repository (`owner/repo`; a default can be baked in with `BS_UPDATE_REPO=owner/repo`
  at build time); a new version is downloaded, verified and installed from inside (see
  [Installing the client](#installing-the-client)).
- **Administration.** Profile, password change, account deletion; the instance admin panel
  (settings, users, storage, registration, federation peers, streaming limits, telemetry,
  updates).

Security notes and known limitations are in [AUDIT.md](AUDIT.md) (Russian).

## Workspace

| Crate | Purpose | Tests |
|---|---|---|
| `bs-types` | Protocol data types (users, spaces, channels, messages, events) | 59 |
| `bs-permissions` | Permission bits, role hierarchy, overrides | 11 |
| `bs-api` | REST client (`/api/*`), Bearer JWT, uploads and downloads, coded errors | 110 |
| `bs-ws` | WebSocket client: heartbeat, reconnect, event parsing | 11 |
| `bs-instances` | Connection to one instance: dispatcher, reconnect, password and token sign-in | 14 |
| `bs-identity` | Login and registration, token storage, per-remote credentials, `UserKey` | 31 |
| `bs-media` | Image and GIF decoding with size and pixel limits | 13 |
| `bs-voice` | Voice and video over LiveKit: own audio pipeline (cpal), mixer, video stage, camera, screen capture, system audio, data-channel signals | 68 |
| `bs-core` | Instance state and cache, event application, notification rules | 72 |
| `bs-app` | The GUI and its entry point | 254 |
| `bs-tui` | Terminal client (`ratatui`), one instance | 35 |
| `bs-contract` | Contract tests against a real Backspace server | 2 |
| `bs-server` | The Rust server: the original's REST API and WebSocket gateway over the same database, see "Server" | 144 |
| `bs-e2ee` | End-to-end encryption for DMs: X25519, HKDF-SHA256, XChaCha20-Poly1305, safety number | 7 |
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

The package `bs-app` has a single binary called **`rustspace`**, `bs-tui` has **`bs-tui`**.
A release build ends up in `target/release/` (a debug build — without `--release` — in
`target/debug/`).

### Run what you built

```sh
./target/release/rustspace               # Linux and macOS
.\target\release\rustspace.exe           # Windows
./target/release/bs-tui                  # terminal client
./target/release/bs-tui --allow-insecure # bs-tui: allow http outside loopback
```

The binary is self-contained: copy it anywhere and run it. `rustspace` takes no
command-line options; everything is set in the app (*Settings*). Only the environment is
read: `RUST_LOG` for logging, e.g.
`RUST_LOG=rustspace=debug,bs_voice=debug ./target/release/rustspace`. The login form asks
for the instance address, username and password; for an `http://` instance outside loopback
tick "Allow insecure connection (http)".

To put it on your `PATH` instead:

```sh
cargo install --path crates/bs-app --features voice,camera,desktop   # → ~/.cargo/bin/rustspace
rustspace
```

### Build and run in one step (for development)

`cargo run` builds the package and starts its binary (`rustspace` for `bs-app`), with the
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

`~/.config/rustspace/` on Linux (honours `XDG_CONFIG_HOME`),
`~/Library/Application Support/rustspace/` on macOS, `%APPDATA%\rustspace\` on Windows:
`session.json` (instance address; the token is in the keyring), `prefs.json` (theme, scale,
voice, sounds, notifications, hotkeys, language), `locales/` (your own translations).
Settings from older versions (`backspace-rs/`) and their keyring entries are moved on the first
start. On Linux the client also puts `io.github.quddymusic.Rustspace.desktop` and its icon into
`~/.local/share` if no package installed them: desktop portals (global shortcuts) only accept an
application they can find there.

### Packaging

The release workflow builds everything from the table in
[Installing the client](#installing-the-client): the archive with `install.sh`, `.deb`
(`dpkg-deb`), AppImage (`linuxdeploy`), the Windows installer (Inno Setup,
`packaging/windows/rustspace.iss`) and the macOS `.app`. Arch users build
`packaging/arch/PKGBUILD`. The icon and `.desktop` file are in `packaging/linux`.

### Versions and releases

Rustspace has its own version — `version` in the workspace `Cargo.toml`, shared by the client
and the server. The server reports it in `/api/instance/info` as `version`, and the Backspace
version whose protocol it implements as `compatibleWith`.

To release, bump the version, add its section to `CHANGELOG.md` (it becomes the release notes),
commit, and push a tag with the same number:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` checks that the tag matches `Cargo.toml`, builds the server
(static, Linux x86_64 and aarch64 — runs on Debian, Ubuntu and Alpine) and the client (Linux
x86_64 and aarch64, Windows, macOS on Apple Silicon), and creates a **draft** release with the
archives and their `.sha256` files. Check it and press *Publish*: the client's update check
and `setup.sh --release` only see published releases.

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
environment variables and the same SQLite database. An original database file opens as is
(same migrations; applied ones are not repeated), and the other way round: a database
`bs-server` has worked with opens in the original. Tokens and password hashes are shared.

**Ported:** registration and login, invites; profile, password, account deletion; spaces,
members, roles, bans, channels, categories, permission overrides; messages, reactions, read
positions, pokes, search; direct and group conversations; friends and people search;
notification settings; voice channels, calls, voice moderation, LiveKit tokens; file uploads
(tus), downloads, thumbnails, storage cleanup; link previews; space directory and join
requests; stickers and GIF; admin (users, storage, instance settings); serving the web
client; from federation: request signatures, the outbox and delivery to peers (messages,
edits, reactions, read positions, groups, friendships, profile, presence; retries with
backoff, unreachable peers, refusals by the receiver), receiving events from peers (the same events the other way,
attribution checks, replicas of other instances' users, profile picture and attachment
downloads, deleting a replica at its home's request), `user@domain` lookup and friend requests across
instances, signing in to a replica, attaching and re-attaching accounts, the handshake and peer management
(approval, denial, secret rotation, detecting a reinstalled remote instance). Every route
and event is compared with the original by the scenarios in `tools/compat`, on SQLite and on
PostgreSQL.

**Not in the original (extensions):** PostgreSQL storage, two-factor authentication, a key
directory for end-to-end encrypted DMs. They do not change existing routes: a client that
does not know about them works as before. Their tables live in the same database under
`bs_*` names; the original server ignores them.

**Federation** is ported and audited: events, calls and typing flow to and from peers,
`user@domain` lookup works and peers catch up after downtime through the mutation log. The
server never sends telemetry. See [ROADMAP.md](ROADMAP.md).

**Small differences from the original:** thumbnails and stickers are encoded as lossless
WebP (larger files); AVIF thumbnails are not made; video and audio need `ffmpeg` and
`ffprobe` in `PATH` (without them a file is accepted without duration or thumbnail, as in
the original).

### Build

```sh
cargo build --release -p bs-server      # target/release/rustspace-server
```

Only a C compiler is needed (SQLite is built with the server). No Node.js.

### Installing on a server (one script)

`packaging/server/setup.sh` installs, updates and migrates a server on Debian, Ubuntu or
Alpine (systemd or OpenRC), with PostgreSQL and with voice, video and streams through LiveKit.
Without cloning the repository (takes the latest release):

```sh
curl -fsSL https://raw.githubusercontent.com/QuddyMusic/Rustspace/main/packaging/server/setup.sh \
  | sudo sh -s -- --domain chat.example.org
```

or from a clone (builds the server from source):

```sh
git clone https://github.com/QuddyMusic/Rustspace && cd Rustspace
sudo sh packaging/server/setup.sh --domain chat.example.org
```

What it does:

- installs packages and **PostgreSQL**, and creates the `rustspace` database and role in it
  (`--sqlite` keeps everything in SQLite, `--database-url` points at an external PostgreSQL);
- builds `rustspace-server` from the clone, or downloads it from a release (`--release
  [VERSION]`) and checks its checksum, or takes `--binary`;
- downloads LiveKit and checks its hash;
- writes `/etc/rustspace/rustspace.env` and `/etc/rustspace/livekit.yaml`, creates the
  `rustspace` and `livekit` services and checks that both answer;
- **updates**: run it again without arguments (after `git pull`, or with `--release`) — the
  domain, secrets and data are kept, and the database is dumped to `/var/backups/rustspace`
  first (the last five copies are kept);
- **migrates**: an SQLite database found on the way — an installation that used SQLite, a
  server started by hand (it is taken over and stopped), `--data-from DIR` — is copied to
  `/var/lib/rustspace` and moved into PostgreSQL with `rustspace-server import-sqlite` in a
  single transaction; the SQLite file is then kept as `*.migrated`. If a run is interrupted,
  the next one continues. An installation with the old names (`/etc/backspace`,
  `/var/lib/backspace`, the `backspace` service and user) is moved to the new ones and the old
  ones are removed.

It is meant for a reverse proxy in front (Nginx Proxy Manager, Caddy…): at the end it prints
what to set there and on the router — `https://DOMAIN` → port 3000, `https://lk.DOMAIN` → port
7880 (both with WebSockets), TCP 7881 and UDP 7882 forwarded to the machine. `--help` lists all
options.

### Run

```sh
JWT_SECRET="$(openssl rand -hex 32)" \
DB_PATH=data/rustspace.db UPLOAD_DIR=data/uploads \
target/release/rustspace-server
```

The server listens on `0.0.0.0:3000`. The first registered user becomes the administrator.
It stops on `Ctrl+C` or `SIGTERM`, waiting for open requests.

Moving from the original server: stop it, copy the database and start `rustspace-server`
with the same `JWT_SECRET`, `DB_PATH` and `UPLOAD_DIR`. Going back works the same way.

To have the server also serve the original web client, build it
(`pnpm --filter @backspace/web build`) and point at the build: `BS_WEB_DIST=/path/to/packages/web/dist`.
Without it, `/` shows a short page saying the instance is reached with the Rustspace desktop
client, with its address to copy and a link to the source and builds (`BACKSPACE_SOURCE_URL`,
by default this repository; set your own if you changed the server). The page is in Russian
for browsers that prefer Russian and in English for everyone else (`?lang=ru|en` switches). The icon on that page is
`packaging/brand/rustspace/icon.svg`; the other variants and their generator are next to it.

### PostgreSQL

```sh
createdb -T template0 -E UTF8 --lc-collate=C --lc-ctype=C backspace
DATABASE_URL=postgres://user:password@127.0.0.1/rustspace JWT_SECRET=… target/release/rustspace-server
```

The server creates and extends the schema itself: it is derived from the original
migrations. The `C` collation makes string order and comparison match SQLite. To move data
from a SQLite file (of the original server or of `bs-server`) into an empty PostgreSQL
database:

```sh
DATABASE_URL=postgres://… JWT_SECRET=… target/release/rustspace-server import-sqlite data/rustspace.db
```

The import is one transaction: all or nothing. There is no import back. TLS to PostgreSQL
follows libpq's `sslmode` in the address: `disable`; `prefer` (the default: encrypted when the
server offers it, the certificate not checked); `require` (encrypted only; the certificate is
checked when `sslrootcert` is given); `verify-ca` (the chain up to a trusted authority);
`verify-full` (the chain and the host name). `sslrootcert` is a PEM file of authorities or
`system`; without it `verify-*` trust the system roots. Across a network use `verify-full`:

```sh
DATABASE_URL='postgres://user:password@db.example/backspace?sslmode=verify-full&sslrootcert=/etc/ssl/db-ca.pem'
```

The server uses one connection: transactions run one at a time, as with SQLite.

### Two-factor authentication

A user turns it on in account settings (the Rust client) or through `/api/auth/2fa`: `setup`
(password) returns a secret for an authenticator app, `enable` (code) turns protection on
and returns ten recovery codes. After that, login needs a `totp` or `recoveryCode` field;
without it the server answers `401` with code `two_factor_required`. The secret is stored
encrypted with a key derived from `JWT_SECRET`: after changing `JWT_SECRET`, protection has
to be set up again. Someone who lost both the phone and the recovery codes is let back in by
the administrator: a password reset in the admin panel also removes the second factor.

### Bridges to Telegram, Discord and Matrix

*(bs-server)* A text channel can be linked to a Telegram group, a Discord channel or a
Matrix room: messages from the channel go there, messages from there come into the channel.
People from another network appear as separate users named like "Anna · Telegram"; they
cannot log in. Files go out as links to the server.

1. Create a bot and put its token into `/etc/rustspace/rustspace.env`, then restart the
   server (`systemctl restart rustspace` or `rc-service rustspace restart`). The installer
   keeps these variables on updates.
   - **Telegram**: create a bot with @BotFather, set `TELEGRAM_BOT_TOKEN`. Turn privacy mode
     off (`/setprivacy` → Disable), otherwise the bot only sees commands.
   - **Discord**: create an application at discord.com/developers, add a bot, turn on
     *Message Content Intent*, set `DISCORD_BOT_TOKEN`. Invite the bot with the permissions
     to view channels, send messages and *Manage Webhooks* (with webhooks Discord shows the
     author's name instead of the bot's).
   - **Matrix**: register an account for the bot, set `MATRIX_HOMESERVER`
     (`https://matrix.org`) and `MATRIX_ACCESS_TOKEN` (Element: Settings → Help & About →
     Access Token). Encrypted rooms are not supported.
2. Add the bot to the chat and send `/rustspace_id` (Telegram) or `!rustspace_id` (Discord,
   Matrix): it replies with the chat ID. A Matrix bot joins rooms it is invited to.
3. In the Rust client open the channel settings → **Bridges**, pick the network and paste
   the ID. Linking needs the "Manage channels" permission.

`TELEGRAM_API_URL` and `DISCORD_API_URL` change the API address (a local Bot API server,
tests).

### Webhooks and cards

*(bs-server)* Channel settings → **Webhooks** creates an address an outside program writes to
without logging in: CI, monitoring, RSS, your own bot. The body is the same as Discord's
(`content`, `username`, `avatar_url`, `embeds` with title, description, colour, fields,
footer, image), so ready integrations work once the address is swapped:

```sh
curl -H 'Content-Type: application/json' \
  -d '{"content":"Build **passed**","embeds":[{"title":"main","color":3066993}]}' \
  https://your.domain/api/webhooks/ID/TOKEN
```

`?wait=true` returns the created message. The address is a secret: delete the webhook if it
leaks. Managing webhooks needs "Manage channels". Cards are stored as ordinary embeds, so the
original web client shows their title and text too.

### Activity and game icons

*(bs-server)* Activities may carry `playback` (`positionMs`, `durationMs`): a track position
that does not depend on anyone's clock; the server moves it forward while it holds the
activity, so a late `ready` shows the current point. An activity disappears as soon as the
connection that published it closes (the desktop app quit, the browser tab stays), and
removing one is accepted inside the 3-second rate limit. `POST /api/game-icons` takes a
base64 PNG, JPEG or WebP (up to 512 KB), stores a 256 px WebP and returns its URL; an icon
nobody uploaded for a year is cleaned up. Ported from Backspace PR #482.

### Saved Messages, pins, bookmarks and 18+ channels

*(bs-server)* Additions over the original API (`/api/rustspace/…`; bookmarks at
`/api/bookmarks` follow the API proposed for the original in PR #474); original clients simply
don't see them. Saved Messages is a group DM with its owner as the only member, so the
original web client shows it as an ordinary group. Pins live in `bs_pins` (scope `me` or
`all`), bookmarks in `bs_bookmarks`, the 18+ flag in `bs_channel_flags` and in the channel
object as `nsfw`. Events: `rs_pins_updated`, `bookmark_added`, `bookmark_removed`.

### Voice bitrate

*(bs-server)* The original publishes the microphone with LiveKit's default bitrate. Here the
instance admin sets it in *Settings → Streaming → Voice in calls* (16–510 kbit/s, 96 by
default); whoever manages a voice channel, and both sides of a DM call, can lower it for that
call when the connection is poor. The client applies a change on the fly.

### Configuration

Environment variables are the same as the original's; those specific to `bs-server` are
marked.

| Variable | Default | Meaning |
|---|---|---|
| `JWT_SECRET` | none (required) | Token signing key, at least 32 characters |
| `JWT_EXPIRES_IN` | `30d` | Token lifetime (`30d`, `12h`, `3600`) |
| `PORT`, `HOST` | `3000`, `0.0.0.0` | Where to listen |
| `DB_PATH` | `data/rustspace.db` (`data/backspace.db` if only that exists) | SQLite database file |
| `DATABASE_URL` | none | *(bs-server)* PostgreSQL address `postgres://…` (TLS: `sslmode`, `sslrootcert`); when set, `DB_PATH` is not used |
| `UPLOAD_DIR` | `data/uploads` | Directory for uploaded files |
| `MAX_UPLOAD_SIZE` | `104857600` | File size limit in bytes (the administrator can change it in settings) |
| `TUS_EXPIRATION_HOURS` | `24` | How long an unfinished upload lives |
| `BS_WEB_DIST` | none | *(bs-server)* Directory of the built web client; without it `/` shows a "use the desktop client" page and the rest is API only |
| `DOMAIN` | none | Instance domain (invite links, `user@domain` names) |
| `PUBLIC_ORIGIN` | none | Full external address when it is not `https://DOMAIN` |
| `TRUSTED_PROXY_HOPS` | `1` | How many proxies in front write `X-Forwarded-For` (0–4). Rate limits depend on it: behind one nginx `1`, without a proxy `0` |
| `REGISTRATION_OPEN` | `true` | Whether registration without an invite is open (the administrator can change it) |
| `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` | none | LiveKit server for voice; without them voice is off |
| `DIRECTORY_ENDPOINT` | project directory | Space directory address; an empty string turns it off. The server contacts it only if the administrator enabled the directory |
| `BACKSPACE_UPDATE_CHECK` | `false` | Let the admin panel ask GitHub about a new release of the original (the original defaults to `true`) |
| `BACKSPACE_INSTALL_CHANNEL` | none | `prebuilt` or `source`: what the update panel shows |
| `DISABLE_RATE_LIMITS` | `false` | Turn rate limits off (tests only) |
| `BS_EXTRA_CA_CERTS` | none | *(bs-server)* PEM file with extra root certificates for requests to other instances (like the original's `NODE_EXTRA_CA_CERTS`) |
| `FEDERATION_BACKOFF_DIVISOR` | `1` | Divide the waits between delivery attempts (tests only) |
| `FEDERATION_ALLOW_PRIVATE_PEERS` | `false` | Let peers whose address was not given by an admin use private addresses (tests only) |
| `DISABLE_FEDERATION_WORKERS` | `false` | Do not run background peer checks |
| `BACKSPACE_SOURCE_URL` | this repository | Source of the running server (AGPL §13): instance info and `/` |
| `BACKSPACE_COMMIT` | none | Shown in instance info |
| `BS_PASSWORD_HASH_COST` | `12` | *(bs-server)* bcrypt cost (4–16); change for tests only |
| `RUST_LOG` | `info` | *(bs-server)* Log verbosity (`debug`, `bs_server=trace`) |

Put a TLS reverse proxy in front that passes WebSocket on `/ws`.

### Compatibility check

`tools/compat` starts the original server and `bs-server` on copies of one database and runs
the same scenarios on both, comparing responses and WebSocket events:

```sh
tools/compat/up.sh                    # needs a Backspace checkout with `pnpm install`
python3 tools/compat/s_spaces.py      # and s_channels, s_messages, s_dm, s_social, s_account,
                                      # s_voice, s_admin, s_search, s_files, s_embeds,
                                      # s_stickers, s_gif, s_explore, s_directory,
                                      # s_adminusers, s_bodies; cmp_ready.py
python3 tools/compat/t_twofactor.py   # bs-server extensions: two-factor
python3 tools/compat/t_e2ee.py        # and the key directory
BS_E2EE_LIVE_A=https://x1.test BS_E2EE_LIVE_B=https://y1.test BS_E2EE_LIVE_CA=/tmp/fedca/ca.pem \
  cargo test -p bs-app e2ee_live -- --ignored   # two clients over federation (fresh fed_up.sh stand)
```

The checkout path is set by `ORIG` (see the top of `up.sh`). The same run with `bs-server` on
PostgreSQL: `BS_STAND_PG=postgres://user@host:port tools/compat/up.sh`; the `bs_stand`
database is recreated and filled from the same SQLite copy.

Federation is checked on a separate stand of two instance pairs: original ↔ original and
`bs-server` ↔ original (with `BS_FED_RUST_Y=1`, `bs-server` on both sides; with
`BS_FED_PG=postgres://…`, `bs-server` on PostgreSQL). The same scenario runs on both pairs;
responses, database rows (peers, the outbox, the mutation log), what the second instance
received, what the first one took in (replicas, the applied-event ledger, the file queue), and
events are compared. The original's federation addresses an instance by its
host name without a port, so the instances reach each other over https as `x1.test`,
`y1.test`, `x2.test`, `y2.test`: `fed_up.sh` adds them to `/etc/hosts`, issues a test CA and
starts the `tls_front.py` TLS front on port 443 (root is needed).

```sh
tools/compat/fed_up.sh                # instances on ports 3111–3114, fresh databases
python3 tools/compat/f_peering.py peering    # each scenario on a fresh stand
tools/compat/fed_up.sh && python3 tools/compat/f_peering.py approvals
tools/compat/fed_up.sh && python3 tools/compat/f_outbox.py delivery
FEDERATION_BACKOFF_DIVISOR=100000 tools/compat/fed_up.sh && python3 tools/compat/f_outbox.py retries
tools/compat/fed_up.sh && python3 tools/compat/f_inbound.py inbound relay_checks
tools/compat/fed_up.sh && python3 tools/compat/f_lookup.py friends accounts
BS_FED_TEST_ROUTES=1 FEDERATION_BACKOFF_DIVISOR=1 tools/compat/fed_up.sh && python3 tools/compat/f_sync.py pull serve pages
BS_FED_LIVEKIT=1 tools/compat/fed_up.sh && python3 tools/compat/f_calls.py one_on_one group
```

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

## Acknowledgements

Rustspace stands on the work of the people behind Backspace:

- **TheZwiss** and the Backspace contributors — the original project: protocol, data model,
  server logic, web client and the game dictionary.
- **QuddyMusic** and **cirzear** — activity and Rich Presence: the local IPC server, game and
  media detection, game icons (Backspace PR #482; part of that code was written with Z.ai).
- **cirzear** — message translation in Backspace (PR #448), the starting point for the
  translator here.
- **QuddyMusic** — the bookmarks API (Backspace PR #474) that Rustspace's bookmarks follow.

## License

Rustspace is licensed under the [GNU AGPL-3.0-only](LICENSE), the same license as the original
Backspace, from which its protocol, data model and server logic are ported. Copyright notices,
the relationship to Backspace and third-party licenses are listed in [NOTICE](NOTICE). Security
reports: [SECURITY.md](SECURITY.md).

If you run a modified `bs-server` for other people, the AGPL requires you to offer them its
source: set `BACKSPACE_SOURCE_URL` to your repository (by default it points here).

The Rustspace "R" mark is this project's own; the Backspace name and logo belong to their owner
and are not used here.

Third-party assets: the [Inter](https://rsms.me/inter/) font (`crates/bs-app/assets/fonts/`,
SIL Open Font License 1.1); interface sounds from the original Backspace
(`crates/bs-app/assets/sounds`, AGPL-3.0-only); syntax grammars and colour themes from
[bat](https://github.com/sharkdp/bat) via `two-face` (each under its own licence, see that crate);
[Twemoji](https://github.com/twitter/twemoji) emoji images (`crates/bs-app/assets/emoji`,
CC-BY 4.0, © Twitter, Inc and other contributors).
