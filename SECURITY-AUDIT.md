# Security audit

*Русская версия: [SECURITY-AUDIT.ru.md](SECURITY-AUDIT.ru.md).*

Scope of this pass: the client (`bs-api`, `bs-ws`, `bs-identity`, `bs-media`, `bs-voice`,
`bs-app`) and its dependencies. The server section is filled in as the Rust server is written.

## Client — fixed in this pass

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| C1 | Medium | Links supplied by an instance (source-code URL, release page, anything rendered as a hyperlink) were passed to the OS opener unchecked. The opener accepts any scheme — `file:`, `smb:`, application handlers — so a hostile or compromised instance could trigger them with one click. | One gate at the end of every frame drops every `OpenUrl` that is not `http(s)://` with a host and without credentials (`app::safe_link`), wherever it came from. |
| C2 | Medium | The LiveKit address comes from the instance and was used as is: an instance reached over `https` could move a call to plaintext `ws://`. | `ws://` is accepted only for loopback or when the instance itself is opened over `http://` (which the user has already allowed); otherwise the call is refused with a message. |
| C3 | Low | Dependencies with fixed advisories: `lru` (RUSTSEC-2026-0253), `rustix` (RUSTSEC-2026-0486); yanked `async-executor`, `clipboard-win`. | Updated in `Cargo.lock`. |

## Client — checked, no change needed

- **Tokens.** The home token lives in the OS keyring; the file fallback and the federation
  token store are written atomically with `0600` and `create_new`; secrets are zeroized and
  never logged.
- **Transport.** `https`/`wss` only unless the user explicitly allows plaintext for an
  instance; redirects are off in the API client; WebSocket frames and messages are size-capped.
- **Token scope.** Each instance has its own client and token; asset requests carry no token;
  absolute asset URLs must be same-origin; external images are limited to an allow-list of GIF
  hosts over `https` with validated redirects.
- **Files.** Download names are stripped of directories and special characters; uploads and
  images go through size and dimension limits before decoding (`bs-media::Limits`).
- **Message content.** Markdown only auto-links `http(s)://`; there is no HTML rendering.
- **Updates.** The checker only reads the GitHub API over TLS and opens the browser; it never
  downloads or runs anything itself.
- **Unsafe code.** Denied workspace-wide (`forbid` in `bs-voice`).

## Client — accepted or open

- `quick-xml` 0.38 (RUSTSEC-2026-0194/0195, DoS on hostile XML) comes from `syntect`/`plist`
  and `zbus_xml`. It only parses data bundled with the build, never network input. Waiting for
  upstream.
- `glib` 0.18 (RUSTSEC-2024-0429, unsound `VariantStrIter`) comes with GTK 3 used by the tray.
  The affected iterator is not used. It goes away with a GTK 4 tray.
- Unmaintained transitive crates (`bincode`, `paste`, `proc-macro-error`, `ttf-parser`,
  `yaml-rust`, `zvariant` 4): no known vulnerabilities; tracked with `cargo audit`.
- A partially downloaded file is written next to the target as `<name>.part`; in a
  world-writable folder another local user could pre-create it. Low risk, not changed.
- Display names and link text are shown as sent, including Unicode direction overrides. The
  full address is always in the tooltip; stripping overrides is planned with the server-side
  validation.

## Server

The Rust server (`bs-server`) is written against the original server's code, so each part of
the original is reviewed as it is ported. Findings so far (configuration, storage, sign-up,
sign-in, tokens):

| # | Severity | Finding in the original | In the Rust server |
|---|----------|-------------------------|--------------------|
| S1 | Low | Sign-in answers for an unknown username before any password hashing, so response time tells whether a username exists. | bcrypt always runs (against a dummy hash when there is no user); timing no longer distinguishes the cases. |
| S2 | Low | Unhandled exceptions are returned with their message (Fastify default), which can carry SQL or paths. | Internal errors are logged and answered with a fixed body. |
| S3 | Low | Snowflake ids follow the wall clock: if it steps back, ids go back and may repeat. | The generator never goes back in time; ids stay increasing and unique. |
| S4 | Info | `account_deleted` is answered for a deleted account whatever the password, revealing that the account existed. | Kept for compatibility (clients show a specific message); timing is equalised. |
| S5 | Info | bcrypt only uses the first 72 bytes of a password. | Same format on purpose: hashes stay interchangeable with the original. |
| S6 | High | The LiveKit token route and `voice_join` checked only `CONNECT`. A private channel is closed by denying `VIEW_CHANNEL` while `CONNECT` stays granted, so a member could get a token for a hidden voice room and listen in. | Fixed: both rights are required; a member who loses access is removed from the room. |
| S7 | Medium | The recipient of a friend request could answer it again: its status was not checked. A request accepted long ago could be "accepted" once more, restoring a friendship the other side had ended (and with it the right to add them to groups and send invites). | Fixed: only a pending request can be answered. |
| S8 | Medium | A kicked or departed member kept their role rows and personal permission overrides and got them back on rejoining by invite, moderator roles included. | Fixed: roles and personal overrides are removed with the membership. |
| S9 | Medium | An author could edit their messages after being kicked or banned: only authorship was checked. | Fixed: editing requires that the author still sees the channel. |
| S10 | Medium | A kicked or banned member stayed subscribed to the space's events until reconnecting. | Fixed: the subscription ends right after `member_left`. |
| S11 | Medium | Voice events (`voice_state_update`, statuses, moderator mutes) went to the whole space, revealing who sits in a hidden channel and when. | Fixed: only members who can view the channel receive them. |
| S12 | Medium | `channel_ack` parsed an id of any length as a `BigInt`: a megabyte of digits stalled the server's only thread, repeatable twice a second. | Fixed: ids are at most 20 digits. |
| S13 | Low | Reaction strings had no length limit, and a reaction could be added in a channel the member cannot view. | Fixed: at most 256 characters, and view access is required. |
| S14 | Low | `POST /api/users/@me/verify-password` had no rate limit. | Fixed: same limit as login. |
| S15 | Low | An attachment with no recorded uploader could be attached by anyone who knew its id. | Fixed: only the uploader can attach it. |
| S16 | Info | Moderator mute/deafen and disconnect are carried out by the client: the LiveKit session is not torn down, so a modified client stays in the room until its token expires (one hour). | Fixed for disconnects: `bs-server` removes the participant from the room through the LiveKit API itself (moderator disconnect, lost permission, deleted channel). Mutes are still enforced when a token is issued. |
| S17 | Info | Changing the password does not close open WebSocket connections (REST tokens are revoked). A replicated account needs no current password to change it. | As in the original; closing connections risks logging out a client that has not stored its new token yet. |
| S18 | Medium | The upload size limit was checked only at creation: an upload with deferred length (`Upload-Defer-Length`) accepted a file of any size. | Fixed: the limit applies when the length is declared later and to the data stream itself. |
| S19 | Low | `GET /api/uploads/.tus` (a directory instead of a file) answered 500 with the file system error text. | Fixed: 404. |
| S20 | Medium | The link-preview guard against requests into the internal network validated the address with a separate DNS lookup and then connected by name again (DNS answer swap between check and connect); only the first address was checked; IPv6 addresses embedding IPv4 (NAT64, 6to4) counted as public. | Fixed: the name is resolved once, every address is checked, the connection goes to the checked address, and each redirect hop is checked the same way. |
| S21 | Low | Accepting a join request did not check a ban issued after it was filed; a request from someone who had already joined gave a 500. | Fixed. |
| S22 | Info | The original has no two-factor authentication: a stolen password is enough to sign in. | Added to `bs-server` as an extension (TOTP and recovery codes); the secret is stored encrypted, a code is not accepted twice, wrong codes are limited per account. |

Kept exactly as in the original: bcrypt hashes (cost 12), HS256 tokens with the same fields
(tokens and password hashes work on both servers with the same `JWT_SECRET`), revocation of
tokens issued before a password change, per-address rate limits behind a bounded number of
trusted proxies, security headers.
