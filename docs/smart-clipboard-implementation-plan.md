# Smart clipboard — implementation plan

Companion to `smart-clipboard-prd.md` (requirements) and
`smart-clipboard-architecture.md` (design). This document is the build plan: stack,
repo layout, and a phase-by-phase breakdown with concrete tasks and verification
checklists. Phases match the tables in PRD §9 and architecture §6 exactly.

Guiding constraint: **phases 1–2 are the dogfoodable MVP** — capture + local search
+ minimal MCP. Everything after that improves a product that is already in daily use
by its author.

---

## 1. Stack summary

| Concern | Choice | Notes |
|---|---|---|
| App shell | Tauri v2 | Rust core + webview UI; sub-1% idle CPU target rules out Electron |
| Database | `rusqlite` with `bundled-sqlcipher-vendored-openssl` feature | Whole-DB encryption; WAL mode |
| Vector index | `sqlite-vec` (Rust bindings, loaded as extension) | 384-dim vectors |
| Local embeddings | `fastembed` crate, model `bge-small-en-v1.5` (384-dim, quantized ONNX) | Runs in the Rust core, not the webview |
| MCP server | `rmcp` (official Rust MCP SDK), stdio transport | Lives in the core process |
| Key storage | `keyring` crate | OS keychain (Windows Credential Manager / macOS Keychain) |
| IDs | `ulid` crate | Time-ordered, sync-friendly |
| Clipboard | `tauri-plugin-clipboard-manager` + raw Win32 `AddClipboardFormatListener` (via `windows` crate) on Windows; NSPasteboard polling/observer on macOS | Raw listener needed to read conceal formats before deciding to capture |
| Global hotkey | `tauri-plugin-global-shortcut` | With foreground-window check for the Alt+C ownership rule |
| OCR | `windows` crate → `Windows.Media.Ocr` (Win); `objc2` + Vision framework (macOS) | Local, free; Groq vision optional later |
| Date parsing (NL queries) | `chrono` + `chrono-english` (or equivalent) | Local "yesterday"/"last week" parsing |
| HTTP (Groq) | `reqwest` + `serde_json` | OpenAI-compatible client shape so the provider is swappable |
| UI | React + TypeScript + Vite (Tauri webview) | Palette, session view, privacy dashboard |
| Browser extension | Manifest V3, TypeScript | Chrome first, Firefox after |

## 2. Repository layout

```
smart-clipboard/
├── src-tauri/                  # Rust core
│   ├── src/
│   │   ├── main.rs             # Tauri setup, tray, window mgmt
│   │   ├── capture/
│   │   │   ├── clipboard.rs    # OS hooks, conceal-flag gate, exclusion gate
│   │   │   ├── hotkey.rs       # Alt+C, ownership rule, dedupe window
│   │   │   ├── hud.rs          # capture HUD overlay window
│   │   │   └── native_msg.rs   # native-messaging host for the extension
│   │   ├── pipeline/
│   │   │   ├── fast_tier.rs    # entity classify, secret heuristics, dedupe, FTS write
│   │   │   ├── entities.rs     # taxonomy regexes/heuristics
│   │   │   ├── secrets.rs      # entropy + pattern detectors
│   │   │   ├── embed.rs        # fastembed worker, type-enriched input builder
│   │   │   ├── ocr.rs          # OS-native OCR (per-platform impls)
│   │   │   └── groq.rs         # classify/tag/summarize client + queue worker
│   │   ├── sessions/
│   │   │   ├── scoring.rs      # time-gap + source-affinity assignment
│   │   │   ├── sweep.rs        # idle close + nightly jobs (incl. adaptive threshold)
│   │   │   └── finalize.rs     # dedupe, cluster, summary trigger
│   │   ├── db/
│   │   │   ├── mod.rs          # connection mgmt, SQLCipher key, migrations
│   │   │   ├── schema.sql
│   │   │   └── queries.rs
│   │   ├── search/
│   │   │   ├── query_parser.rs # local NL → structured filter
│   │   │   └── retrieval.rs    # FTS + vec + filter combination, ranking
│   │   ├── mcp/
│   │   │   └── server.rs       # rmcp tools, exposure policy, access logging
│   │   └── export/
│   │       └── formats.rs      # markdown briefing, context packs, JSON
│   └── Cargo.toml
├── ui/                         # React app (search palette, sessions, dashboard)
├── extension/                  # MV3 extension (TS)
└── docs/
```

---

## 3. Phase 0 — de-risk spike (~week 1, throwaway code allowed)

The whole storage design rests on three unproven integrations. Prove them before
building features on top.

**Tasks**
1. Single Rust binary that opens a SQLCipher-encrypted DB via `rusqlite`
   (bundled-sqlcipher feature), loads the `sqlite-vec` extension into that same
   connection, creates the FTS5 table with `tokenize='trigram'`, and round-trips
   data through all three.
2. `fastembed` spike: first-run model download + local cache location, embed a
   string, insert `FLOAT[384]` into `vec0`, query nearest neighbors, sanity-check
   ranking on ~20 hand-written samples (include a type-enriched color, a code
   snippet, a URL).
3. Windows clipboard listener spike: `AddClipboardFormatListener`, enumerate formats
   on copy, confirm `ExcludeClipboardContentFromMonitorProcessing` is visible when
   copying from a password manager (test with Bitwarden and/or 1Password).
4. Quick `Windows.Media.Ocr` call on a screenshot PNG to confirm API availability on
   Windows 10 (this machine's OS) and output quality on code-heavy screenshots.

**Exit criteria** — all met 2026-07-03, see `phase0-findings.md`:
- [x] One `rusqlite` connection with SQLCipher + sqlite-vec + FTS5-trigram all
      working together (the DB file is unreadable without the key; trigram substring
      match works; KNN query returns sane order).
- [x] Embed→insert→query round-trip under ~50ms per item after model warm-up
      (measured: 9.5ms warm).
- [x] Password-manager copy is detectably flagged on Windows (format-level
      simulation; real-password-manager confirmation still owed).
- [x] Native OCR returns usable text from a screenshot (sufficient for search
      recall; not for verbatim code extraction — as designed).
- **Fallback decisions if a leg fails:** SQLCipher conflict → encrypt at the
  filesystem level or accept plaintext DB v1 with documented risk; sqlite-vec
  conflict → brute-force cosine over an embeddings BLOB column (fine at personal
  scale); trigram unavailable → unicode61 with custom `tokenchars` + LIKE fallback.

---

## 4. Phase 1 — capture + local search (the foundation)

**Goal:** every copy on the machine is captured (through the privacy gates),
classified, embedded, and findable in a palette within 200ms — fully offline.

**Tasks**
1. **Tauri shell** (`main.rs`): tray icon, hidden-by-default main window, autostart
   toggle, single-instance guard.
2. **DB layer** (`db/`): migrations runner, schema from architecture §3.3, SQLCipher
   key created on first run and stored via `keyring`, WAL mode, one writer + pooled
   readers.
3. **Clipboard capture** (`capture/clipboard.rs`): event listener; gate 0 (conceal
   formats), gate 1 (exclusion list from config file); text payloads end-to-end;
   image payloads → WebP asset + `assets` row (OCR arrives in phase 5, but store the
   asset now).
4. **Fast tier** (`pipeline/fast_tier.rs`, `entities.rs`, `secrets.rs`):
   - entity taxonomy: hex/RGB color, email, phone, IP, UUID, coordinates, currency,
     date, file path, URL, code-with-language (heuristic: keywords/syntax density),
     plain text — table-driven so adding a type is one entry + fixtures;
   - secret detection: entropy threshold on token-like strings, known key formats
     (`sk-`, `ghp_`, `AKIA…`, JWT shape, PEM headers), PII patterns; sets
     `sensitivity`, secrets excluded from FTS/embedding entirely;
   - dedupe hash (content + source, recent window);
   - synchronous FTS5 write.
5. **Embedding worker** (`pipeline/embed.rs`): background task in the core; consumes
   new captures, builds type-enriched input (`"hex color code: #3B82F6"`), writes to
   `captures_vec`. Skips `sensitivity = 'secret'`.
6. **Search palette** (UI + `search/retrieval.rs`): global shortcut (e.g.
   Alt+Space), merged FTS + vector results with recency boost, entity-type filter
   chips, copy-back-to-clipboard on Enter.
7. **Capture HUD** (`capture/hud.rs`): frameless always-on-top overlay, shows
   capture summary, auto-dismiss ~2s. (In phase 1 it confirms ambient capture is
   working; its session line lights up in phase 3.)

**Verification** — run 2026-07-03 against the live dev app:
- [x] Copy text anywhere → captured synchronously; retrieval measured 29–35ms
      (fully local path, no network involved).
- [x] Semantic query works: copied `#3B82F6`, "what blue color did I copy" →
      rank 1.
- [x] Substring search works on code: `AuthStore` finds `useAuthStore();` at
      rank 1 (matched by FTS+vec).
- [x] Conceal-flagged copy (password-manager mechanism, simulated via
      `clipset.exe`) → gate 0 DROP in live logs, nothing stored. Real
      password-manager confirmation still owed (none installed on dev machine).
- [ ] Exclusion list live test pending — gate 1 code path is in place with
      config defaults (KeePass/1Password/Bitwarden); needs a run with a real
      excluded app in the foreground.
- [x] Copied a `ghp_…` token → stored `sensitivity=secret`, zero FTS index hits,
      zero vector rows (verified by direct DB dump).
- [x] DB file header is opaque ciphertext; wrong-key open rejected (phase-0
      test); key lives in Windows Credential Manager.
- [x] ~4s cumulative CPU over several minutes including model load — sub-1%
      idle. Capture ran with the main window never shown (hidden by default).
- Note: palette/HUD rendering verified by build only — visual pass owed on
  first interactive use (Alt+Space).
- Note: `source_app` attribution resolves the HUD window when copies originate
  from a headless shell; verify attribution during interactive dogfood use.
- Unit tests: table-driven fixtures for every entity type and secret pattern
  (positive + negative cases); integration test on a temp DB for the full
  capture→search path.

---

## 5. Phase 2 — minimal MCP server (ship the differentiator)

**Goal:** Claude Code can search the clipboard history. This is deliberately early —
it's the product's reason to exist, and dogfooding it now shapes everything after.

**Tasks**
1. `mcp/server.rs` on `rmcp`, stdio transport; a small launcher mode
   (`smart-clipboard --mcp`) that connects to the same DB so agents can attach
   whether or not the tray app is running — single-writer discipline via WAL.
2. Tools: `search_context(query, limit)` → runs the same retrieval path as the
   palette (FTS + vector), returns text, entity type, source app/URL, timestamp;
   `list_recent_captures(limit, since)`.
3. Exposure policy v1 (hard-coded, conservative): `sensitivity = 'public'` only,
   never secrets; every call appended to `access_log`.
4. Register in Claude Code (`claude mcp add`) and use it for real work.

**Verification**
- [ ] From Claude Code: "search my clipboard for the sqlite-vec crate docs link" →
      correct capture returned.
- [ ] Secrets and private-sensitivity items never appear in MCP results (seed test
      data and assert).
- [ ] Every MCP call visible as an `access_log` row.
- One week of real dogfood use; capture friction notes feed phase 3.

---

## 6. Phase 3 — sessions + browser extension

**Goal:** Alt+C captures with browser context, auto-grouped into concurrent
sessions.

**Tasks**
1. **Scored assignment** (`sessions/scoring.rs`): for each open session, score =
   w_t·f(time gap) + w_s·(source affinity: same registrable domain / same app);
   assign best-above-threshold else open new session; weights and threshold in
   config. Multiple open sessions supported (the PRD's interleaved-work case).
2. **Sweep** (`sessions/sweep.rs`): periodic task closes idle-past-threshold
   sessions; nightly job placeholder for adaptive threshold (learning lands in
   phase 4, logging starts now).
3. **Alt+C path** (`capture/hotkey.rs`): global shortcut → foreground-window check —
   if a known browser, yield to the extension (ownership rule); else capture
   selection (synthesized copy) + window title, through the normal gates.
4. **Browser extension** (`extension/`): MV3, `chrome.commands` Alt+C; content
   script grabs selection + surrounding DOM text + URL/title;
   `chrome.tabs.captureVisibleTab()` screenshot; background service worker relays
   via native messaging.
5. **Native messaging host** (`capture/native_msg.rs`): host manifest
   registration (per-browser, installer step), JSON protocol
   (`{text, context_before, context_after, url, title, screenshot_b64}`), dedupe
   window (content hash, ±2s) against the global-hotkey path.
6. **HUD upgrade:** shows assigned session ("captured → *rust sqlite encryption*"),
   click to reassign/discard; reassignments write `session_corrections`.
7. **Session view** (UI): list sessions with captures; merge/split/rename/reassign —
   all writing `session_corrections`.

**Verification**
- [ ] Alt+C on a browser selection → capture has URL, title, surrounding text,
      screenshot asset; HUD <150ms with session name.
- [ ] Alt+C in a non-browser app → capture has app + window title; no double-fire
      when a browser is focused (assert single row).
- [ ] Interleaved test: research captures on topic A, two Slack copies, back to A →
      A's session intact, tangent in its own session.
- [ ] 30-min gap → next Alt+C opens a new session; sweep closes the old one without
      any new capture.
- [ ] Merge/split in the session view works and logs corrections.
- Unit tests: scoring function (table-driven scenarios: gap boundaries, domain
  affinity vs. gap tradeoffs); native-messaging protocol round-trip.

---

## 7. Phase 4 — Groq enrichment + smart search

**Goal:** session captures get topics/tags, misfiled captures get flagged, and the
palette understands "what color did I copy yesterday" — offline.

**Tasks**
1. **Groq client** (`pipeline/groq.rs`): OpenAI-compatible chat client (`reqwest`),
   so provider is a base-URL + model-name config (Groq default; anything
   OpenAI-compatible works). Structured JSON output: topic, tags, project,
   topic-affinity score vs. the session's `affinity_hint`.
2. **Queue worker:** consumes `enrichment_queue` (session captures only, per FR11),
   exponential backoff on 429/network, resumes on reconnect; every call logged to
   `access_log` with byte counts; hard pre-send assert: `sensitivity != 'secret'`.
3. **Affinity re-bucketing:** low-affinity captures flagged; finalize pass (phase 6)
   proposes reassignment; session `affinity_hint` updated as tags accumulate.
4. **Adaptive threshold** (`sessions/sweep.rs` nightly): nudge time-gap threshold
   from `session_corrections` (splits of auto-merged sessions → threshold too high;
   merges of auto-split sessions → too low). Simple hill-climb with floor/ceiling;
   log every adjustment.
5. **Local NL query parser** (`search/query_parser.rs`): grammar over the fixed
   entity taxonomy + `chrono-english` date ranges + source hints ("from github") →
   structured filter combined with vector ranking. No network.
6. **Optional RAG synthesis:** explicit "ask" affordance in the palette — one Groq
   call over the top-k retrieved captures, answer with capture citations; logged.

**Verification**
- [ ] Alt+C capture → enrichment lands async: topic/tags visible; capture was never
      blocked.
- [ ] Kill network mid-queue → capture/search unaffected; queue drains after
      reconnect with backoff (inspect `next_attempt_at`).
- [ ] "what color did I copy yesterday" → correct result, **offline**.
- [ ] Seeded secret in a session → assert no Groq request is ever built for it
      (unit-test the pre-send gate).
- [ ] `access_log` rows match actual outbound calls 1:1.
- Unit tests: query-parser fixtures (20+ NL queries → expected filters); backoff
  schedule; threshold-adjustment logic.

---

## 8. Phase 5 — screenshots + native OCR

**Goal:** every screenshot and Alt+C tab capture is searchable by the text inside it.

**Tasks**
1. `pipeline/ocr.rs`: Windows impl via `windows` crate (`Windows.Media.Ocr`); macOS
   impl via `objc2` + Vision (`VNRecognizeTextRequest`); trait-based so platforms
   and a future Groq-vision option plug in behind one interface.
2. Wire into the local async tier: asset stored (phase 1) → OCR → `assets.ocr_text`
   (`ocr_source = 'native'`) → FTS + type-enriched embedding ("screenshot text: …").
3. Ambient image copies get the same treatment (FR4a) — still no Groq.
4. Retention guardrails now, not later: WebP quality setting, per-asset size cap,
   configurable retention window for ambient-image assets, storage-usage readout in
   settings.
5. Optional config-gated Groq vision path stub (off by default; if ever enabled,
   re-check Groq's current production vision models — preview tier churns).

**Verification**
- [ ] Alt+C on a browser tab → screenshot OCR'd; searching a phrase visible only in
      the image finds the capture.
- [ ] Copy an image (e.g. snip tool) → asset stored, OCR text searchable, no Groq
      traffic (assert empty `access_log` delta).
- [ ] 100-screenshot soak: storage growth matches the compression settings; OCR
      queue keeps up without UI jank.

---

## 9. Phase 6 — finalize + export + paste conveniences

**Goal:** a closed session becomes a hand-off-ready briefing; the app earns daily
paste-side usage.

**Tasks**
1. **Finalize job** (`sessions/finalize.rs`): near-dup collapse (normalized-hash +
   embedding-similarity threshold), sub-topic clustering (greedy over embeddings —
   cheap and local), apply pending affinity re-bucketing flags, then one Groq
   summary call (`llama-3.3-70b-versatile`) with the curated capture set.
2. **Export formats** (`export/formats.rs`):
   - markdown briefing: summary on top, clustered excerpts, per-excerpt source
     attribution (URL/title/timestamp);
   - context packs: ~4k / ~16k token variants (greedy fill by cluster
     representativeness; token counts estimated locally);
   - JSON: full structured session.
   Export to clipboard or file.
3. **Compaction job:** for ambient captures older than the retention window and
   below a value floor (never searched/opened, no session): drop `raw_text`,
   keep entity type + embedding + metadata; tombstone via `deleted_at` where
   appropriate. No Groq involvement by design.
4. **Paste conveniences (FR20):** paste-as-plain-text (global shortcut or palette
   action); quick-paste of recent items from the palette (Enter pastes into the
   previously focused app). Paste stack/transforms remain v2.

**Verification**
- [ ] Close a 20-capture session with duplicates → export has deduped, clustered,
      attributed excerpts + summary; every excerpt carries a source.
- [ ] 4k pack actually fits ~4k tokens (check with a tokenizer).
- [ ] Paste the briefing into Claude/ChatGPT and ask questions — the attribution
      survives round-trip (the actual product moment).
- [ ] Compaction dry-run mode lists what it *would* drop; after running, semantic
      search still surfaces compacted items by their embedding.

---

## 10. Phase 7 — full MCP + privacy dashboard

**Goal:** agents pull whole sessions under an explicit policy; the user can audit
everything that ever left the device.

**Tasks**
1. MCP tools: `get_session(id)` (finalized markdown or structured JSON),
   `list_recent_sessions(limit)`; `search_context` gains entity/time filter params
   (reusing the phase-4 query parser).
2. **Exposure policy, config-backed:** defaults — finalized sessions only,
   `sensitivity='public'` only, secrets never; opt-ins for in-progress sessions and
   private items; per-tool enable/disable.
3. **Access logging everywhere** + optional first-time consent prompt when a new MCP
   client connects.
4. **Privacy dashboard** (UI over `access_log`): timeline of every Groq call and MCP
   read — what, when, to whom, bytes; filter by actor; storage stats and "what would
   compaction do" alongside.
5. Hardening pass: MCP results carry explicit provenance framing (content from
   untrusted web pages is data, not instructions); fuzz the native-messaging and MCP
   inputs; review error paths for content leaks into logs.

**Verification**
- [ ] Claude Code: "pull my latest research session" → attributed briefing arrives
      via MCP, no copy-paste.
- [ ] Policy tests: in-progress session blocked by default then allowed after
      opt-in; private/secret items never returned under any config.
- [ ] Dashboard reconciles 1:1 with a proxy/netlog capture of actual Groq traffic.
- [ ] End-to-end PRD walkthrough: all six §5 use cases demonstrated.

---

## 11. Testing strategy (cross-phase)

- **Unit (Rust):** table-driven fixtures for `entities.rs`, `secrets.rs`,
  `scoring.rs`, `query_parser.rs` — these four carry most correctness risk and are
  pure functions; keep fixtures growing from real dogfood misses.
- **Integration:** temp-DB tests covering capture→FTS→embed→search,
  queue backoff, finalize, compaction; a seeded-DB fixture generator for UI work.
- **E2E (manual, scripted checklist per phase):** the verification lists above; run
  the previous phases' checklists on each phase boundary (regression).
- **Dogfood metrics (PRD §8):** correction rate, search success, hand-off latency,
  HUD latency — reviewed at each phase boundary; they decide tuning priorities.
- **Privacy assertions as tests, not conventions:** "secret never leaves" and
  "ambient never hits Groq" are asserted in code paths (pre-send guards) *and*
  covered by tests that attempt to violate them.

## 12. Sequencing and definition of done

- Phases are strictly ordered 0→7; within a phase, tasks are roughly ordered but can
  interleave.
- A phase is done when its verification checklist passes and the previous phases'
  checklists still pass.
- **MVP = phases 0–2** (capture, local search, minimal MCP): the app must be in
  daily personal use from the end of phase 2 onward — later phases are steered by
  that usage, not by this document alone.
- Revisit-flags recorded for v2: paste stack + transforms, cross-device sync (schema
  already ready), Firefox extension port, Linux/Wayland scope, optional Groq-vision
  image understanding, accessibility-API context capture for native apps.
