# FastSearch Roadmap

**v0.1 (this build)** — portable walker index (std::fs), in-memory search
(glob / ANDed substring tokens), FSIDX1 persistence, handle-based C ABI.
Correctness-first baseline; works on any OS, indices are only as fresh as
the last build.

**v0.2 — NTFS fast paths (Windows)**
- USN Journal incremental updates: after the initial build, replay the
  journal so the index stays current without re-walking.
- Optional MFT enumeration for the initial build on NTFS volumes
  (admin-gated, falls back to the walker with the reason recorded).
- Change notification helper so hosts can refresh on demand.

**v0.3 — search quality**
- Delivered ahead of this phase: explicit extension, path-substring and
  file/directory filters through an additive C ABI (no query syntax changes).
- Remaining: size/date ranges and optional extension filter syntax.
- Fuzzy matching (opt-in), ranked results mode alongside the stable
  sorted mode.

**Later**
- Multiple roots per index; watch mode as an explicit opt-in API
  (never background work the host did not ask for).
