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
- **Two-factor authentication** (on a server that supports it, that is `bs-server`): a code at
  login; turning on, recovery codes and turning off in account settings.
- **End-to-end encryption for one-to-one conversations.** Turned on from the conversation
  header; text is encrypted on the device (X25519, XChaCha20-Poly1305) and travels as an
  ordinary message, so it works through any server, the original included. Keys are exchanged
  in the conversation itself and verified with a safety number. Attachments and group
  conversations are not encrypted; there is no forward secrecy. The key lives in the system
  keyring and moves to another device as a passphrase-protected record (the encryption
  window). The other person needs this client: the original web client shows ciphertext.
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
| `bs-permissions` | Permission bits, role hierarchy, overrides | 11 |
| `bs-api` | REST client (`/api/*`), Bearer JWT, uploads and downloads, coded errors | 110 |
| `bs-ws` | WebSocket client: heartbeat, reconnect, event parsing | 11 |
| `bs-instances` | Connection to one instance: dispatcher, reconnect, password and token sign-in | 14 |
| `bs-identity` | Login and registration, token storage, per-remote credentials, `UserKey` | 31 |
| `bs-media` | Image and GIF decoding with size and pixel limits | 13 |
| `bs-voice` | Voice and video over LiveKit: own audio pipeline (cpal), mixer, video stage, camera, screen capture, system audio, data-channel signals | 68 |
| `bs-core` | Instance state and cache, event application, notification rules | 72 |
| `bs-app` | The GUI and its entry point | 234 |
| `bs-tui` | Terminal client (`ratatui`), one instance | 35 |
| `bs-contract` | Contract tests against a real Backspace server | 2 |
| `bs-server` | The Rust server: the original's REST API and WebSocket gateway over the same database, see "Server" | 127 |
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
cargo build --release -p bs-server      # target/release/backspace-server
```

Only a C compiler is needed (SQLite is built with the server). No Node.js.

### Run

```sh
JWT_SECRET="$(openssl rand -hex 32)" \
DB_PATH=data/backspace.db UPLOAD_DIR=data/uploads \
target/release/backspace-server
```

The server listens on `0.0.0.0:3000`. The first registered user becomes the administrator.
It stops on `Ctrl+C` or `SIGTERM`, waiting for open requests.

Moving from the original server: stop it, copy the database and start `backspace-server`
with the same `JWT_SECRET`, `DB_PATH` and `UPLOAD_DIR`. Going back works the same way.

To have the server also serve the original web client, build it
(`pnpm --filter @backspace/web build`) and point at the build: `BS_WEB_DIST=/path/to/packages/web/dist`.

### PostgreSQL

```sh
createdb -T template0 -E UTF8 --lc-collate=C --lc-ctype=C backspace
DATABASE_URL=postgres://user:password@127.0.0.1/backspace JWT_SECRET=… target/release/backspace-server
```

The server creates and extends the schema itself: it is derived from the original
migrations. The `C` collation makes string order and comparison match SQLite. To move data
from a SQLite file (of the original server or of `bs-server`) into an empty PostgreSQL
database:

```sh
DATABASE_URL=postgres://… JWT_SECRET=… target/release/backspace-server import-sqlite data/backspace.db
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

### Configuration

Environment variables are the same as the original's; those specific to `bs-server` are
marked.

| Variable | Default | Meaning |
|---|---|---|
| `JWT_SECRET` | none (required) | Token signing key, at least 32 characters |
| `JWT_EXPIRES_IN` | `30d` | Token lifetime (`30d`, `12h`, `3600`) |
| `PORT`, `HOST` | `3000`, `0.0.0.0` | Where to listen |
| `DB_PATH` | `data/backspace.db` | SQLite database file |
| `DATABASE_URL` | none | *(bs-server)* PostgreSQL address `postgres://…` (TLS: `sslmode`, `sslrootcert`); when set, `DB_PATH` is not used |
| `UPLOAD_DIR` | `data/uploads` | Directory for uploaded files |
| `MAX_UPLOAD_SIZE` | `104857600` | File size limit in bytes (the administrator can change it in settings) |
| `TUS_EXPIRATION_HOURS` | `24` | How long an unfinished upload lives |
| `BS_WEB_DIST` | none | *(bs-server)* Directory of the built web client; without it only the API is served |
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
| `BACKSPACE_SOURCE_URL`, `BACKSPACE_COMMIT` | none | Shown in instance info |
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

## License

AGPL-3.0-only, like the original Backspace.

Third-party assets: the [Inter](https://rsms.me/inter/) font (`crates/bs-app/assets/fonts/`,
SIL Open Font License 1.1); interface sounds from the original Backspace
(`crates/bs-app/assets/sounds`, AGPL-3.0-only); syntax grammars and colour themes from
[bat](https://github.com/sharkdp/bat) via `two-face` (each under its own licence, see that crate).
