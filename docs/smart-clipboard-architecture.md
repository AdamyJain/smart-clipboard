# Smart clipboard / personal context manager — system architecture

## 1. Overview

A background desktop app that captures clipboard activity and, on demand, research
sessions (via a dedicated hotkey), enriches that content using Groq-hosted models,
stores everything locally in an encrypted SQLite database, and exposes it both to the
user (search palette) and to external AI agents (via a local MCP server).

Core principles carried through every layer:
- **Capture is always local and instant.** No network dependency for the act of copying.
- **Background work never depends on the UI process.** Embeddings, the enrichment
  queue, the session sweep, and the MCP server all live in the Rust core — closing
  the window must not stop enrichment or agent access.
- **Embeddings and OCR run locally; heavier reasoning runs through Groq.** Vector
  search works over the full local history at no cost or network dependency; OCR is
  OS-native (free, private, no deprecation risk); Groq is reserved for
  classification, tagging, and summarization, scoped to session captures.
- **Secret/PII detection happens locally, before anything reaches Groq.** Non-negotiable.
- **Ambient copies and session captures are treated differently.** Only explicit,
  intentional captures (Alt+C) trigger the Groq pipeline by default.
- **Everything that leaves the device is logged.** One append-only access log feeds
  a privacy dashboard — the trust story is auditable, not asserted.

**Stack decision (resolves an earlier contradiction):** the core is **Rust-native**.
Earlier drafts mixed Node-only libraries (`better-sqlite3`, `transformers.js`, the
TypeScript MCP SDK) into a Tauri core; those are explicitly rejected — they would
require a Node sidecar process and tie background work to a second runtime. Instead:
`rusqlite` (bundled SQLCipher), `sqlite-vec` (Rust bindings), `fastembed` (local
embeddings), `rmcp` (MCP server). TypeScript remains for the React UI only.

**Extension-free capture (design revision, 2026-07-03):** an earlier draft used a
browser extension + native messaging for in-browser Alt+C context. Dropped — one
install, one shortcut owner, no per-browser packaging. Browser context now comes
from the OS side: screenshot + native OCR for surrounding context, UI Automation
for the address-bar URL. Tradeoff: OCR context is noisier than DOM text (fine for
search recall, not for citation), and there is no DOM-exact `context_before/after`.

---

## 2. Component map

```
┌─────────────────────────────────────────────────────────────────────┐
│ Local machine                                                        │
│                                                                      │
│  ┌──────────────────────┐         ┌───────────────────────────────┐ │
│  │ Foreground window    │◄────────│ Tauri app — Rust core         │ │
│  │ (any app / browser)  │ Alt+C:  │ - clipboard hook + gates      │ │
│  │ - synthesized Ctrl+C │ SendInp │   (conceal flags, exclusions) │ │
│  │ - GDI screenshot     │ + GDI   │ - global hotkey (all apps)    │ │
│  │ - UIA address-bar    │ + UIA   │ - fast tier (classify/secret) │ │
│  │   URL (browsers)     │         │ - native OCR (context)        │ │
│  └──────────────────────┘         │ - embedder (fastembed)        │ │
│                                   │ - session scoring engine      │ │
│                                   │ - enrichment queue worker     │ │
│                                   │ - MCP server (rmcp)           │ │
│  ┌──────────────────────┐         │ - capture HUD (overlay win)   │ │
│  │ React/TS webview     │◄───IPC──┤                               │ │
│  │ UI ONLY:             │         └──────┬─────────────────┬──────┘ │
│  │ - search palette     │                │                 │        │
│  │ - session view       │       ┌────────▼───────┐  ┌──────▼──────┐ │
│  │ - privacy dashboard  │       │ SQLite         │  │ Groq client │ │
│  └──────────────────────┘       │ (SQLCipher,    │  │ - classify/ │ │
│                                 │  whole-DB enc) │  │   tag +     │ │
│                                 │ - captures     │  │   affinity  │ │
│                                 │ - sessions     │  │ - session   │ │
│                                 │ - FTS5 trigram │  │   summary   │ │
│                                 │ - sqlite-vec   │  │ - RAG synth │ │
│                                 │ - access_log   │  │ - vision    │ │
│                                 │ - assets       │  │   (optional)│ │
│                                 └────────────────┘  └─────────────┘ │
│                                          │                          │
│                                          │ (MCP server, stdio/local)│
└──────────────────────────────────────────┼──────────────────────────┘
                                           │
                            ┌──────────────▼───────────────┐
                            │ External agents / apps       │
                            │ Claude Code, Claude Desktop, │
                            │ ChatGPT (via export)         │
                            └──────────────────────────────┘
```

OS-native OCR (Windows.Media.Ocr / Apple Vision) is called from the Rust core; it is
local and does not appear on the network edge of this diagram by design.

---

## 3. Layers

### 3.1 Capture layer

| Piece | Tech | Notes |
|---|---|---|
| OS clipboard hook | `tauri-plugin-clipboard-manager` / `arboard`, plus raw Win32 `AddClipboardFormatListener` on Windows | Fires on every copy; pushes a raw event through the gate chain below |
| **Gate 0: conceal flags** | Check `ExcludeClipboardContentFromMonitorProcessing` (Windows) / `org.nspasteboard.ConcealedType` (macOS) **before anything else** | Password managers set these on every copy — zero-config correct behavior with 1Password/Bitwarden/KeePass; dropped content never enters any queue |
| Gate 1: per-app exclusion list | Config-driven, checked before capture queues | Second line of defense for apps that don't set conceal flags (banking apps, etc.) |
| Global hotkey | `tauri-plugin-global-shortcut` | Alt+C is the single dedicated shortcut — every intentional capture, every app, one owner (no extension) |
| **Alt+C selection leg** | Synthesized Ctrl+C via `SendInput` (releasing the held Alt first), guarded by `GetClipboardSequenceNumber` | If the sequence number never changes there was no selection — stale clipboard content is never captured. The ambient listener sees the synthesized copy first; the richer hotkey event upgrades that row in place (dedupe path, incl. FTS refresh) |
| **Alt+C context leg** | GDI screenshot of the foreground window → PNG asset → `Windows.Media.Ocr` | OCR text becomes searchable context (`context_before`); with no selection it becomes the capture itself — non-selectable content (images, PDFs, video frames) is capturable |
| **Alt+C URL leg (browsers)** | UI Automation: address-bar Edit control's Value pattern | Works on Chrome/Edge/Firefox/Brave without an extension; best-effort (scheme restored for stripped omnibox values, incl. `file:///` drive paths) |
| **Capture HUD** | Small Tauri overlay window, <150ms, auto-dismiss | Shows what was captured + which session it joined; click-through to reassign or discard. This is what makes capture feel trustworthy instead of haunted |
| Image payloads | Copied images stored as content-addressed assets | OCR'd locally (see 3.2), searchable via OCR text; never sent to Groq by default |
| Session assignment | Scoring engine in Rust core — see 3.4 | Multiple sessions may be open concurrently; no single rolling pointer; zero-affinity captures only join within a short burst window |

### 3.2 Preprocessing layer — two-tier pipeline

**Fast tier (synchronous, local, no network)** — runs on every capture, ambient or session:
1. Content-type + entity classification (regex/heuristics), expanded beyond a coarse
   code/URL/text split into a real taxonomy: hex/RGB colors, emails, phone numbers,
   IP addresses, UUIDs, coordinates, currency amounts, dates, file paths, and
   code-with-detected-language. This runs on ambient captures too, at no extra cost —
   it's what makes a bare value like `#3B82F6` recognizable as "a color" at all.
2. Secret/PII detection (layer 3 of 3 — conceal flags and the exclusion list already
   ran at capture): entropy heuristics, API-key format patterns, PII patterns.
   Decision: store raw / redact / exclude from any future Groq call.
3. Dedupe check against recent hash
4. Write to SQLite + FTS5 — item is keyword-searchable within milliseconds

**Local async tier (embeddings + OCR, no cost, no network)** — runs on every capture,
ambient or session, in the Rust core (never the webview — enrichment must survive the
window closing):
1. Embedding generation via `fastembed` (`bge-small-en-v1.5`, 384-dim — small,
   modern, noticeably better retrieval than the older MiniLM class), written to the
   `sqlite-vec` index — swappable for a cloud provider (e.g. OpenAI
   `text-embedding-3-small`) via config if quality matters more than the cost/latency
   tradeoff for a given user
2. The text handed to the embedding model is a type-enriched representation, not the
   raw value alone — e.g. `"hex color code: #3B82F6"` rather than just `"#3B82F6"`.
   A bare hex string carries almost no semantic signal on its own; the entity type
   recognized in the fast tier is what makes it retrievable by a natural-language
   query like "what color did I copy."
3. OCR for image payloads and screenshots: **OS-native** — `Windows.Media.Ocr` via
   the `windows` crate on Windows, Apple Vision via an objc2 bridge on macOS. Free,
   local, private, no rate limits, no preview-model deprecation risk. OCR text is
   indexed (FTS + embedding) like any other capture text.

This tier is *not* gated to session-only captures — because it costs nothing and
never leaves the device, the entire clipboard history becomes semantically
searchable, not just research sessions.

**Groq async tier (cloud, costs money, rate-limited)** — runs only for session-tagged
captures by default (configurable to include ambient captures if the user wants that
later):
1. Structured classify+tag call — `llama-3.1-8b-instant`, JSON output: topic, session
   tags, project tag, plus a topic-affinity score against the assigned session (see 3.4)
2. Session summarization at session end — `llama-3.3-70b-versatile`
3. *Optional, config-gated:* Groq vision for layout/semantic understanding of images
   where plain OCR text isn't enough. Never the default OCR path; if enabled,
   re-check Groq's current production model list (preview-tier vision models are
   liable to change).

Every Groq call writes a row to `access_log` (what, when, how many bytes) — this
feeds the privacy dashboard.

The queue is a persisted SQLite table (`enrichment_queue`), processed by a worker loop
with exponential backoff on rate-limit or network errors. Capture never blocks on this.

### 3.3 Storage layer

SQLite via `rusqlite` with the bundled **SQLCipher** feature — **whole-database
encryption**, key stored in the OS keychain (`keyring` crate). Single file, WAL mode
enabled for concurrent read/write between the UI and the background worker.

> Design note: an earlier draft encrypted individual columns (`raw_text`,
> `context_*`) with Stronghold. Rejected: the FTS5 index built over those same
> columns stores plaintext tokens, so per-column encryption protects nothing (or
> breaks search if you index ciphertext). Whole-DB encryption covers the FTS and
> vector indexes automatically and is simpler.

IDs are **ULIDs** (time-ordered, globally unique) and deletes are **soft**
(`deleted_at` tombstones) — cross-device sync is v2, but the schema is sync-ready
from day one so v2 doesn't start with a migration.

```sql
-- core capture record
CREATE TABLE captures (
  id TEXT PRIMARY KEY,              -- ULID
  session_id TEXT REFERENCES sessions(id),
  captured_at INTEGER NOT NULL,
  content_type TEXT,                -- code | url | text | secret | image
  entity_type TEXT,                 -- color | email | phone | ip | uuid | coord |
                                    -- currency | date | filepath | code:<lang> | url | text
  raw_text TEXT,
  source_app TEXT,
  source_url TEXT,
  page_title TEXT,
  context_before TEXT,
  context_after TEXT,
  sensitivity TEXT DEFAULT 'unknown',   -- public | private | secret
  dedupe_hash TEXT,
  asset_id TEXT REFERENCES assets(id),
  enrichment_status TEXT DEFAULT 'pending',  -- pending | done | skipped
  deleted_at INTEGER                -- soft delete / tombstone
);

-- trigram tokenizer: the corpus is code-heavy; `useAuthStore` and `api_key_id`
-- must be findable by substring, which the default unicode61 tokenizer can't do
CREATE VIRTUAL TABLE captures_fts USING fts5(
  raw_text, context_before, context_after,
  content=captures, content_rowid=rowid,
  tokenize='trigram'
);

-- vector index; embedding source (local fastembed or cloud API) is a config
-- choice, not a schema choice — sqlite-vec stores whatever vector it's given
CREATE VIRTUAL TABLE captures_vec USING vec0(
  capture_id TEXT PRIMARY KEY,
  embedding FLOAT[384]   -- bge-small-en-v1.5 dimension
);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,              -- ULID
  topic TEXT,
  started_at INTEGER,               -- timestamp of first capture
  last_activity_at INTEGER,         -- bumped on every capture; drives gap detection
  ended_at INTEGER,                 -- set on gap-close or nightly sweep
  status TEXT DEFAULT 'open',       -- open | closed  (multiple rows may be open)
  boundary_source TEXT,             -- scored | manual_split | manual_merge
  affinity_hint TEXT,               -- running topic hint / centroid for scoring
  summary TEXT,
  deleted_at INTEGER
);

CREATE TABLE tags (id TEXT PRIMARY KEY, label TEXT, kind TEXT);
CREATE TABLE capture_tags (capture_id TEXT, tag_id TEXT);

CREATE TABLE assets (
  id TEXT PRIMARY KEY,
  file_path TEXT,        -- content-addressed, compressed (WebP)
  ocr_text TEXT,
  ocr_source TEXT        -- native | groq
);

CREATE TABLE enrichment_queue (
  capture_id TEXT PRIMARY KEY,
  attempts INTEGER DEFAULT 0,
  next_attempt_at INTEGER
);

-- every off-device transfer and every agent read, append-only;
-- backs the privacy dashboard (PRD FR18)
CREATE TABLE access_log (
  id TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,     -- groq | mcp
  action TEXT NOT NULL,    -- classify | summarize | vision | search_context | get_session | ...
  ref_id TEXT,             -- capture or session id
  bytes_sent INTEGER
);

-- manual merge/split events; training signal for the adaptive threshold (PRD FR5/FR8)
CREATE TABLE session_corrections (
  id TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  kind TEXT NOT NULL,      -- merge | split | reassign
  from_session TEXT,
  to_session TEXT,
  capture_id TEXT
);
```

Retention: a background compaction job periodically **drops the raw payload** of old,
low-value ambient captures while keeping the entity classification, embedding, and
metadata. (Not "summarize and drop" — summarizing ambient content would mean sending
it to Groq, which the privacy gates forbid. Recall survives via the embedding; the
payload doesn't need to.)

### 3.4 Session / assembly layer

- **Session boundary detection — no explicit start/end.** Alt+C is the only
  shortcut, so session boundaries are inferred, not declared. An earlier draft used
  a single rolling `current_session_id` pointer; rejected because real work is
  interleaved — one Slack-tangent copy mid-research would either pollute the session
  or reset its timer. Instead:
  1. *Local, instant (stage 1 — scored assignment):* for each **open** session,
     compute a score from (a) time gap since that session's `last_activity_at` and
     (b) **source affinity** — same domain cluster, same app, same project directory.
     Assign the capture to the best-scoring session above threshold; otherwise open
     a new session. Multiple sessions can be open concurrently, so the Slack tangent
     lands in its own bucket and the research session's timer is untouched.
  2. *Adaptive threshold:* the time-gap threshold (default ~25 min) is
     user-configurable and self-tuning — every manual merge/split/reassign is logged
     to `session_corrections`, and a nightly job nudges the per-user threshold to
     reduce the correction rate (the PRD's primary boundary-accuracy metric).
  3. *Async, Groq-assisted (stage 2):* the classify/tag call already running in the
     slow tier also returns a topic-affinity score against the assigned session's
     running hint. Low affinity flags the capture for re-bucketing at the next
     finalize pass, rather than blocking capture in real time.
  4. *Nightly sweep:* closes any open session whose `last_activity_at` is older than
     the threshold even if no new capture ever arrives to trigger the close.
  5. *Manual override:* merge/split/rename/reassign controls in the session view —
     and every use of them is a labeled training example for step 2.
- **Finalize job** (runs when a session closes, from any of the triggers above):
  dedupe near-identical captures, cluster by sub-topic, Groq summary call.
- **Search / "ask your clipboard":** three cooperating paths, the first two fully
  offline:
  1. *FTS5 (trigram)* — fast exact/substring path when the user knows the wording,
     including code identifiers.
  2. *Structured NL filters, parsed locally* — "what color did I copy yesterday"
     becomes `entity_type = color AND captured_at ∈ yesterday` via the fixed entity
     taxonomy plus local date-expression parsing (`chrono-english`-style). No
     network round-trip just to understand the query — search works offline, as the
     PRD promises. Combined with vector similarity over `captures_vec` for ranking.
  3. *Groq RAG synthesis (optional, explicit)* — only when the user asks a question
     rather than browsing results, one Groq call synthesizes an answer from the
     already-retrieved candidates. Groq sits downstream of retrieval, never inside it.

### 3.5 Export / exposure layer

- **Manual export:** finalized session → formatter → clipboard or file, in three
  shapes: (a) attributed, deduped **markdown briefing** with the Groq summary at the
  top and source-URL citations (not a raw dump — context rot is real), (b)
  **token-budget context packs** (~4k and ~16k variants) for pasting into
  budget-constrained chats, (c) **JSON** for programmatic consumers.
- **Agent exposure:** MCP server via the `rmcp` crate, running inside the Rust core
  (available whenever the app runs, window open or not). Two-stage rollout:
  - *Minimal (phase 2):* `search_context(query)`, `list_recent_captures()` — ships
    early because the agent hand-off is the product's differentiator and must be
    dogfooded from the first weeks.
  - *Full (phase 7):* adds `get_session(id)`, `list_recent_sessions()`, and the
    **exposure policy**: finalized sessions by default, `sensitivity = 'public'`
    items only, secrets never; every read logged to `access_log`.
- **Privacy dashboard:** UI view over `access_log` — every Groq call and every MCP
  read: what left the device, when, to whom. Cheap to build, central to trust.

---

## 4. Two data-flow walkthroughs

**A — ambient copy (plain Ctrl+C):**
copy event → gate 0 (conceal flags — a 1Password copy dies here, before any queue) →
gate 1 (app exclusion list) → fast tier (entity classify, secret heuristics, FTS5
index) → local embedding generated in the core and written to `sqlite-vec`, so it's
semantically searchable too → scored session assignment (usually: no open session
matches → unsessioned) → no Groq call at all unless the user later explicitly asks a
question that triggers RAG synthesis over search hits.

**B — research session (Alt+C, browser or any app):**
global shortcut fires → screenshot of the foreground window (GDI) + address-bar URL
via UI Automation (browsers) → synthesized Ctrl+C, guarded by the clipboard sequence
number (no selection → the OCR text becomes the capture) → gates 0/1 (the ambient
listener path) → fast tier stores/upgrades the row; screenshot stored as asset and
OCR'd via OS-native OCR into searchable context → scored assignment picks the
matching open session (time + source affinity) or opens one → **HUD flashes:
"captured → session: React auth research"** → embedding written to `sqlite-vec`;
Groq classify/tag runs async and returns topic-affinity for boundary correction
(call logged to `access_log`) → session closes via gap timeout, nightly sweep, or
manual split → finalize job (dedupe, cluster, Groq summary) → exported as
markdown/pack/JSON or pulled by an agent via MCP (read logged).

---

## 5. Security & privacy gates

0. **OS conceal flags** checked before anything else —
   `ExcludeClipboardContentFromMonitorProcessing` (Windows),
   `org.nspasteboard.ConcealedType` (macOS). Password managers get correct behavior
   with zero configuration; flagged content never enters the pipeline.
1. Per-app exclusion list checked **before** anything queues — highest-sensitivity
   apps never enter the pipeline at all.
2. Local secret/PII detector (entropy + patterns) runs before any network call — the
   one step that can't be delegated to Groq, since it's what decides whether Groq
   sees the data at all.
3. Zero Data Retention enabled on the Groq account (available in account settings).
4. **Whole-database encryption** (SQLCipher, key in OS keychain) — covers raw
   content, FTS index, and vector index alike. (Per-column encryption rejected — see
   3.3 design note.)
5. AI enrichment via Groq is opt-in per capture type (session captures by default;
   ambient captures only if the user explicitly turns that on) — controls both cost
   and the amount of data that ever leaves the device. Local embeddings and
   OS-native OCR are exempt from this gate since they never leave the device.
6. **MCP exposure policy + audit:** finalized sessions by default, public-sensitivity
   items only, secrets never; every MCP read and every Groq call appended to
   `access_log` and visible in the privacy dashboard. Note: session content exported
   to an agent originates from untrusted web pages — exports carry clear source
   attribution so consuming agents can treat it as data, not instructions.

---

## 6. Build order

| Phase | Scope |
|---|---|
| 0 | De-risk spike: SQLCipher + FTS5-trigram + sqlite-vec in one Rust connection; fastembed round-trip; conceal-flag detection on Windows |
| 1 | Tauri shell (Rust core), clipboard hook with conceal-flag + exclusion gates, SQLCipher DB + FTS5 (trigram), fast-tier entity classify, secret heuristics, local embeddings (fastembed) + `sqlite-vec`, search palette, capture HUD |
| 2 | **Minimal MCP server** (`search_context`, `list_recent_captures`) — dogfood the differentiator immediately |
| 3 | Alt+C sessions (scored assignment, concurrent sessions), screenshot + OCR context capture, UIA address-bar URL — extension-free |
| 4 | Groq classify/tag pipeline + topic-affinity correction, adaptive threshold learning, local NL query parser + optional RAG synthesis |
| 5 | Screenshot capture + OS-native OCR |
| 6 | Session finalize (dedupe/cluster/summary), exports (markdown / context packs / JSON), paste conveniences |
| 7 | Full MCP (`get_session`, exposure policy, access log) + privacy dashboard |

See `smart-clipboard-implementation-plan.md` for the detailed per-phase task
breakdown, module layout, and verification checklists.
