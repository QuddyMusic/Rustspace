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
