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

Kept exactly as in the original: bcrypt hashes (cost 12), HS256 tokens with the same fields
(tokens and password hashes work on both servers with the same `JWT_SECRET`), revocation of
tokens issued before a password change, per-address rate limits behind a bounded number of
trusted proxies, security headers.
