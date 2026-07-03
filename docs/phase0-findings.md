# Phase 0 — de-risk spike findings

Date: 2026-07-03 · Machine: Windows 10 Home (10.0.19045) · Rust 1.96.1 (stable-msvc)
Spike code: `spikes/phase0/` (five binaries; kept for reference, not production code).

**Verdict: GO on all four legs. No fallback decisions triggered.** The stack named
in the implementation plan works as designed on the target machine.

## Results

| Leg | Result | Evidence |
|---|---|---|
| SQLCipher + sqlite-vec + FTS5 trigram, one connection | **PASS** | SQLCipher 4.5.7 (SQLite 3.45.3) + sqlite-vec v0.1.9 via `sqlite3_auto_extension`; trigram MATCH finds `AuthStore` inside `useAuthStore`; vec0 KNN order correct; DB header opaque, wrong key rejected, correct key reopens |
| fastembed `bge-small-en-v1.5` → vec0 KNN | **PASS** | 384-dim confirmed; **9.5ms/item warm** (criterion ~50ms); KNN <1ms at toy scale; **7/7 NL queries hit rank 1**, including "what blue color did I copy" → `#3B82F6` — validates the type-enriched embedding strategy (FR9a) |
| Clipboard conceal-flag detection | **PASS** | `AddClipboardFormatListener` on a message-only window works; normal copy → CAPTURE verdict; copy carrying `ExcludeClipboardContentFromMonitorProcessing` → DROP verdict |
| Windows.Media.Ocr | **PASS (with caveat)** | 37ms on a 900×300 code screenshot; prose and key phrases recovered cleanly; code text degrades (line reordering, `B`→`8` confusion, dropped underscores) |

## Version pins that worked together

```toml
rusqlite   = { version = "0.32", features = ["bundled-sqlcipher-vendored-openssl"] }
sqlite-vec = "0.1"    # resolved 0.1.9
fastembed  = "4"
windows    = "0.58"   # needs Win32_Graphics_Gdi for RegisterClassW/WNDCLASSW
```

## Notes for phase 1

1. **Clipboard open can transiently fail** while the copying app still holds the
   clipboard — one enumeration attempt returned "could not open clipboard". The
   capture path needs a short retry loop (e.g. 3 × 15ms), which is standard
   clipboard-listener practice.
2. **OCR quality boundary:** native OCR is fully sufficient for search recall
   (trigram FTS tolerates its errors well), not for verbatim code extraction. This
   confirms the architecture's split: native OCR default, Groq vision as the
   optional precision upgrade — no change needed.
3. **First model load is ~15s** (download + ONNX session init); warm loads will be
   faster but still nontrivial — initialize the embedder once at app start in the
   core, never per-capture.
4. **Build prerequisites** (now installed on this machine, needed on any dev/CI
   box): VS 2022 Build Tools (VCTools), Strawberry Perl (OpenSSL configure), NASM
   (optional — openssl-src fell back to `no-asm` without it).
5. **Manual test still owed:** the conceal-flag DROP was verified by setting the
   format programmatically (`clipset.exe`, same mechanism password managers use).
   When a real password manager is available, run
   `spikes/phase0/target/debug/clipwatch.exe 30` and copy a password to confirm.
