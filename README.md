# Smart Clipboard

A privacy-first clipboard manager with local semantic search — every copy on your
machine is captured, classified, and findable in milliseconds, fully offline.

**[⬇ Download for Windows](https://github.com/AdamyJain/smart-clipboard/releases/latest)**

## Why

Clipboard history is a commodity — Win+V already does it. Smart Clipboard captures
*context*: what you copied, where it came from, and what kind of thing it is, then
makes all of it searchable with natural language. Ask *"what blue color did I copy"*
and it finds `#3B82F6`.

## Features (Phase 1)

- **Ambient capture** — every copy is captured automatically in the background,
  classified by entity type (URL, email, color, code, file path, currency, and more).
- **Instant local search** — press `Alt+Space` for a search palette that merges
  full-text and semantic (vector) search with recency boost. Retrieval measured at
  ~30ms, no network involved.
- **Capture HUD** — a small always-on-top overlay confirms each capture, so the app
  never feels like a black box.
- **Privacy gates, on by default:**
  - Conceal-format copies (password managers) are dropped before they're ever stored.
  - Configurable app exclusion list (KeePass, 1Password, Bitwarden pre-configured).
  - Secret detection (API keys, tokens, JWTs, PEM blocks, high-entropy strings) —
    secrets are flagged and excluded from search indexes and embeddings entirely.
- **Encrypted at rest** — SQLCipher database, key stored in Windows Credential
  Manager. Nothing leaves your device.

## Install

1. Download the installer from the [latest release](https://github.com/AdamyJain/smart-clipboard/releases/latest).
2. Run it. The app lives in your system tray — copy things as usual.
3. Press `Alt+Space` to search everything you've copied.

> Windows SmartScreen may warn about an unsigned installer — click
> **More info → Run anyway**. Code signing is planned.

## Build from source

Requirements: Rust (stable), Node.js 18+.

```sh
npm install
npm --prefix ui install
npx tauri build
```

The installer lands in `src-tauri/target/release/bundle/nsis/`.

## Roadmap

- **Phase 2** — MCP server: let Claude Code and other agents search your clipboard context.
- **Phase 3** — automatic research sessions: captures are grouped by topic without manual start/stop.
- Later: OCR on image captures, session summaries, attributed markdown export.

See [docs/](docs/) for the full PRD, architecture, and implementation plan.

## License

MIT
