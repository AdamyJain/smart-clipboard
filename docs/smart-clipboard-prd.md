# PRD — Smart clipboard / personal context manager

## 1. Problem

Knowledge work today involves constant context switching: research spread across a
dozen browser tabs, code snippets and errors copied between an IDE and a terminal,
half-remembered facts that get lost the moment the clipboard is overwritten. Existing
clipboard managers (Raycast, Alfred, Paste, ClipMacs, Deck) solve search over history
well, but none of them capture *why* something was copied — the source, the
surrounding context, or the intent behind the copy. "Record everything" tools like the
original Rewind solved the context problem but did it by continuously recording the
screen, which turned out to be a serious trust liability (Rewind's pivot to a
cloud-based, Meta-acquired product is the cautionary case study here) and is heavy on
storage and CPU.

Separately, using an LLM to help synthesize research currently means manually
re-copying and reformatting everything you found across ten tabs before you can even
ask a question about it.

**Positioning note:** search-over-clipboard-history is a commodity — Raycast, Paste,
and even Windows' built-in Win+V already do it. The defensible core of this product
is the *agent hand-off*: structured, attributed research context that an AI agent can
pull directly. That is why MCP exposure ships early (phase 2), not last.

## 2. Goals

- Capture clipboard content plus its source context (URL, surrounding text,
  screenshot) with a single, low-friction action.
- Give the user immediate, visible feedback on every intentional capture — the app
  must never feel like a black box that may or may not have heard the hotkey.
- Automatically organize captures into research sessions without requiring the user
  to manually start or end anything — including when work is interleaved across
  topics.
- Make captured history searchable instantly, locally, with no network dependency —
  this includes natural-language queries with time and entity-type filters, not just
  keyword search.
- Produce a curated, attributed, summarized context package from a session — not a
  raw dump — ready to paste into an LLM chat or hand to an agent directly via MCP.
- Expose context to agents (MCP) as a first-class, early deliverable — it is the
  differentiator, and it must be dogfoodable from the first weeks of the build.
- Do all of this without compromising on privacy: secrets never leave the device,
  cloud AI use is deliberate and scoped, and the user always knows what's stored
  where — and what left the device, when, and to whom.

## 3. Non-goals (v1)

- Continuous screen/audio recording of any kind (explicitly rejected — see Rewind
  case study above).
- Cross-device sync (but the data model is sync-friendly from day one — see FR19 —
  so v2 sync is not a schema migration).
- Team or multi-user features.
- A general-purpose enterprise context/metadata platform.
- No local **LLM** inference for classification, tagging, or summarization — those
  run through Groq by design. Two local exceptions, neither of which is LLM
  inference: embeddings (FR14) and **OS-native OCR** (FR-OCR — Windows.Media.Ocr /
  Apple Vision are built into the OS, free, and private; Groq vision is an optional
  enhancement, not the default OCR path).
- Full paste-management suite: a paste stack (sequential multi-paste) and
  transform-on-paste (JSON prettify, case conversion, etc.) are v2. Minimal paste
  conveniences are in v1 — see FR20.

## 4. Target user

Primarily: developers and researchers who do frequent multi-source research and
regularly hand context to LLMs — the initial build target is the author's own
workflow. Secondary: knowledge workers generally who lose track of what they copied
and from where.

## 5. Use cases

1. **Ambient recall.** I copied something hours ago and don't remember which site it
   came from. I search my clipboard history and find it by keyword.
2. **Research session capture.** I'm investigating a topic across ~10 sites. I press
   Alt+C on anything relevant as I go. I don't start or name a session — the app
   figures out where it begins and ends on its own. Each Alt+C flashes a small HUD
   confirming what was captured and which session it joined, so I trust it without
   checking.
3. **Session hand-off.** Once I've moved on to something else for a while, the session
   closes automatically, gets deduped and summarized, and I can export it as an
   attributed markdown briefing to paste into ChatGPT or Claude.
4. **Agent-native hand-off.** Working in Claude Code, the agent pulls my current or
   most recent research session directly via MCP instead of me pasting anything.
5. **Sensitive-data safety.** Passwords, API keys, and anything copied from excluded
   apps (password managers, banking sites) are never queued, never sent to Groq, and
   never indexed for semantic search. Password managers that set the standard OS
   conceal flags (1Password, Bitwarden, KeePass, etc.) are excluded automatically
   with **zero configuration** — the exclusion list is a second line of defense, not
   the only one.
6. **Interleaved work.** Mid-research, a Slack ping sends me on a five-minute tangent
   and I copy a couple of unrelated things. The tangent must neither pollute the
   research session nor reset its idle timer — session assignment considers *where*
   a capture came from, not just *when*.

## 6. Functional requirements

### Capture
- FR1: The system shall capture clipboard content on every OS-level copy event
  (ambient capture), across Windows and macOS at minimum, Linux best-effort.
- FR2: The system shall capture, via a single global hotkey (Alt+C), the current
  selection plus source URL, page title, and surrounding text (browser) or window
  title (other apps), plus a targeted screenshot of the active tab/window.
- FR2a: Exactly one component shall own Alt+C at any moment: when a browser is
  focused, the extension's shortcut owns it and relays through native messaging; the
  desktop app's global shortcut yields, with content-hash + time-window dedupe as a
  safety net against double-fire.
- FR2b: Every Alt+C capture shall show an instant HUD/toast (target <150ms): what
  was captured and which session it joined, with click-through to reassign or
  discard. Capture must never feel like a black box.
- FR3: The system shall maintain a per-app exclusion list; captures from excluded
  apps shall never be queued, stored, or processed.
- FR3a: The system shall honor the standard OS clipboard conceal formats —
  `ExcludeClipboardContentFromMonitorProcessing` (Windows) and
  `org.nspasteboard.ConcealedType` (macOS) — as the **first** gate, checked before
  the exclusion list is even consulted. Content carrying these flags is dropped
  before it enters any queue.
- FR4: Capture shall never block on network availability or AI processing.
- FR4a: Image clipboard payloads (e.g. screenshots copied to the clipboard) shall be
  stored as compressed assets (WebP, content-addressed), OCR'd locally via OS-native
  OCR, and made searchable via the OCR text (FTS + embedding). Images are never sent
  to Groq by default.

### Session boundary detection
- FR5: The system shall infer session boundaries automatically, with no explicit
  start/end action. Assignment shall score **time gap + source affinity** (same
  domain cluster, same app) against each currently open session, so that multiple
  sessions may be open concurrently and an interleaved tangent neither pollutes nor
  resets an active research session. The time-gap threshold (default ~25 min) is
  user-configurable and **adaptive**: manual merge/split corrections are logged and
  used to tune it per user over time.
- FR6: The system shall use a Groq-returned topic-affinity score, computed as part of
  the existing classify/tag call, to flag captures for re-bucketing when the local
  scoring likely got it wrong.
- FR7: The system shall run a periodic sweep to close out sessions that have gone
  idle past the threshold even without a new capture arriving.
- FR8: The system shall provide a manual merge/split/rename control for sessions;
  every such correction is recorded as a training signal for FR5's adaptive
  threshold.

### Preprocessing
- FR9: The system shall classify content into a local entity taxonomy — colors,
  emails, phone numbers, IPs, UUIDs, coordinates, currency, dates, file paths,
  code-with-language, URLs, plain text — via regex/heuristics, on every capture
  including ambient ones, and index into full-text search locally and synchronously,
  before any network call. The FTS index shall support code identifiers and
  substrings (trigram tokenization) — `useAuthStore` and `api_key_id` must be
  findable by partial match.
- FR9a: The system shall construct embedding input from a type-enriched
  representation of a capture (e.g. "hex color code: #3B82F6"), not the raw value
  alone, so that structurally recognized values are retrievable by natural-language
  queries.
- FR10: The system shall detect secrets/PII locally and never send content flagged as
  a secret to any external API. Detection is three-layered: (1) OS conceal flags
  (FR3a), (2) the per-app exclusion list (FR3), (3) local entropy and pattern
  heuristics (API-key formats, high-entropy strings, PII patterns).
- FR11: The system shall run classification/tagging via Groq asynchronously, by
  default only for Alt+C session captures (not ambient captures), queued and retried
  with backoff on failure or rate limiting.
- FR11a: The system shall generate embeddings locally for every capture, ambient or
  session, since this step costs nothing and never sends data off-device — the
  session-only gating in FR11 applies to Groq calls specifically, not to embeddings.
- FR11b: The system shall interpret natural-language queries (e.g. "what color did I
  copy yesterday") into a structured filter — entity type, time range — **locally**,
  using the fixed entity taxonomy and local date-expression parsing, combined with
  vector search. Search, including natural-language filtered search, works fully
  offline. Groq is used only for optional downstream answer synthesis (RAG) over
  already-retrieved hits, never as a required step in retrieval.
- FR12: The system shall generate a session summary via Groq once a session closes.
- FR-OCR: Screenshot and image OCR shall run via **OS-native OCR**
  (Windows.Media.Ocr on Windows, Apple Vision on macOS) — free, local, private, no
  rate limits, no model-deprecation risk. Groq vision is available as an optional,
  config-gated enhancement for layout/semantic understanding of images, never the
  default OCR path.

### Storage
- FR13: The system shall store all data locally in a **whole-database encrypted**
  SQLite database (SQLCipher), with the key held in the OS keychain, and no cloud
  storage of raw content. Whole-DB encryption covers the FTS and vector indexes
  automatically — per-column encryption is explicitly rejected because a plaintext
  FTS index over an encrypted column would defeat it.
- FR14: The system shall support semantic search via a vector index (`sqlite-vec`)
  over embeddings generated locally by default (fastembed, `bge-small-en-v1.5`,
  384-dim), with an optional cloud embedding provider configurable per user
  preference; full-text search remains available as a fast exact-keyword path.
- FR15: The system shall periodically compact old, low-value ambient captures by
  **dropping the raw payload while keeping the entity classification, embedding, and
  metadata** — no summarization of ambient data, because summarization would require
  sending ambient content to Groq, which the privacy gates forbid.
- FR19: The data model shall be sync-friendly from day one: ULID identifiers
  (time-ordered, globally unique) and soft deletes/tombstones (`deleted_at`) instead
  of hard deletes. Cross-device sync itself remains v2, but v2 must not require a
  schema migration.

### Export / exposure
- FR16: The system shall export a finalized session as attributed, deduped markdown
  with a summary and source-URL citations, to clipboard or file; additionally as
  token-budget-sized context packs (e.g. ~4k / ~16k token variants) and as JSON for
  programmatic use.
- FR17: The system shall expose context to external AI agents via a local MCP
  server, shipped in two stages: **minimal** in phase 2 (`search_context`,
  `list_recent_captures` — dogfoodable immediately), **full** in phase 7
  (`get_session`, `list_recent_sessions`, exposure policy). Exposure policy:
  finalized sessions by default, `sensitivity = 'public'` items only, and every MCP
  read is logged to the access log (FR18).
- FR18: The system shall provide a **privacy dashboard**: an audit view, backed by an
  append-only access log, of every Groq call and every MCP read — what left the
  device, when, and to whom. This is the trust story made visible.
- FR20: The system shall provide minimal paste conveniences: paste-as-plain-text and
  quick-paste of recent items from the search palette. (Full paste suite — stack,
  transforms — is v2; see non-goals.)

## 7. Non-functional requirements

- **Performance:** idle CPU usage comparable to best-in-class competitors (sub-1%
  target); capture-to-searchable latency under ~200ms for the local fast tier;
  capture HUD visible under ~150ms.
- **Resilience:** full capture and search functionality — keyword, semantic, and
  natural-language structured-filter — with no network connection; AI enrichment
  resumes automatically once connectivity returns.
- **Independence from the UI:** background work (embedding generation, enrichment
  queue, session sweep, MCP server) must run with the UI window closed — it lives in
  the core process, never in the webview.
- **Cost/rate limits:** AI calls scoped to session captures and explicit
  searches/summaries only, to stay within Groq's per-organization rate limits and
  keep marginal cost near zero for typical usage.
- **Privacy:** Zero Data Retention enabled on the Groq account; whole-database
  encryption at rest; no telemetry beyond what's strictly needed for the app to
  function; every off-device transfer auditable (FR18).
- **Portability:** Windows and macOS as first-class targets; Linux support scoped
  down given Wayland's clipboard-access restrictions for background daemons.

## 8. Success metrics

Measured from the app's own data (no external telemetry):

- **Session-boundary correction rate:** manual merges/splits per 100 sessions —
  should trend down as the adaptive threshold (FR5) learns.
- **Search success:** % of history searches where the user opens/copies a result —
  proxy for "found what I was looking for."
- **Hand-off latency:** time from session close to context delivered to an agent
  (export or MCP pull).
- **Capture trust:** % of Alt+C captures whose HUD confirmation rendered within
  150ms; duplicate-press rate (a user pressing Alt+C twice in <2s signals distrust).

## 9. Phasing

| Phase | Scope |
|---|---|
| 0 | De-risk spike: SQLCipher + FTS5-trigram + sqlite-vec in one Rust connection; fastembed round-trip; conceal-flag detection on Windows |
| 1 | Tauri shell (Rust core), clipboard hook with conceal-flag + exclusion gates, SQLCipher DB + FTS5 (trigram), fast-tier entity classify, secret heuristics, local embeddings (fastembed) + `sqlite-vec`, search palette, capture HUD |
| 2 | **Minimal MCP server** (`search_context`, `list_recent_captures`) — dogfood the differentiator immediately |
| 3 | Alt+C sessions (scored assignment, concurrent sessions), browser extension + native messaging, Alt+C ownership rule |
| 4 | Groq classify/tag pipeline + topic-affinity correction, adaptive threshold learning, local NL query parser + optional RAG synthesis |
| 5 | Screenshot capture + OS-native OCR |
| 6 | Session finalize (dedupe/cluster/summary), exports (markdown / context packs / JSON), paste conveniences |
| 7 | Full MCP (`get_session`, exposure policy, access log) + privacy dashboard |

## 10. Risks and open questions

- **Phase-0 integration risk.** SQLCipher + FTS5 trigram + `sqlite-vec` loading
  together in one rusqlite connection is assumed but unproven — this is the very
  first thing to validate (phase 0), before any feature work.
- **Session boundary accuracy.** The scored-assignment model and adaptive threshold
  need real-world tuning; the correction-rate metric (§8) is the signal.
- **Alt+C double-fire.** The global hotkey and the extension shortcut can both fire
  on the same keypress; the ownership rule + dedupe window (FR2a) is the mitigation,
  but per-browser behavior needs testing.
- **Browser extension complexity.** Manifest V3 behavior differs across Chrome and
  Firefox; native messaging setup is per-browser.
- **Screenshot storage growth.** Even bounded, intentional capture accumulates —
  compression and retention policy need to be right from phase 5, not retrofitted.
- **Groq vision (downgraded).** No longer on the critical path — OCR is OS-native.
  Only relevant if the optional Groq-vision enhancement is enabled; preview-tier
  vision models remain liable to change and should be re-checked at that point.
- **Cost at scale.** Current design assumes light personal usage; needs revisiting if
  usage volume grows well beyond a single user's daily research sessions.

## 11. Out of scope for v1

Cross-device sync (schema is sync-ready, the feature is not), team/sharing features,
mobile apps, local LLM inference for classification/tagging/summarization (OS-native
OCR and local embeddings are in scope — they are not LLM inference), continuous
screen recording, paste stack / transform-on-paste, non-English OCR beyond what the
OS-native OCR engines support out of the box.
