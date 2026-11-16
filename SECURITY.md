# Security policy

This policy covers **Rustspace** (`backspace-rs`, `backspace-server`), an unofficial Rust
client and server compatible with Backspace. It does not cover the original Backspace.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

Report it privately through GitHub:
[QuddyMusic/backspace-rs → Security → Report a vulnerability](https://github.com/QuddyMusic/backspace-rs/security/advisories/new).

Include what is affected (client, server, federation, a specific endpoint), the version or
commit, steps to reproduce and the impact you expect. We will acknowledge the report, keep you
updated and credit you in the advisory unless you prefer otherwise.

## What goes where

| The problem is in… | Report to |
|---|---|
| Rustspace code: the Rust client, `bs-server`, `packaging/server/setup.sh` | this repository, as above |
| The original Backspace (TypeScript server, web or Electron client) | the [official Backspace repository](https://github.com/TheZwiss/backspace) and its own `SECURITY.md` |
| A behaviour Rustspace copies from Backspace | both, if you can — the bug probably exists in both |

## Supported versions

Only the latest release and the `main` branch receive security fixes.

## Known limitations

[`SECURITY-AUDIT.md`](SECURITY-AUDIT.md) lists known risks and how they were handled, including federation as
ported from the original. Direct messages are end-to-end encrypted only when encryption is
switched on for the conversation; until then server administrators can read them, and the
client shows a warning above such conversations.
