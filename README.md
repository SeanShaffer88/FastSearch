# FastSearch.dll

**Build an index of a folder tree once — then search it in-memory, from any language that can call C.**

FastSearch is #2 in the Windows DLL Collection (after WinRecovery). It gives
small tools the thing they all end up wanting — "find files by name,
instantly" — without shelling out to a search app or re-walking the disk
on every keystroke.

## Quick start (C)

```c
#include "fastsearch.h"
uint64_t h = fs_index_create();
fs_index_build(h, "C:\\Users\\me\\Documents");
char *hits = fs_search_json(h, "quarterly report", 20);   // AND tokens
// [{"path":"...\\quarterly report.xlsx","name":"quarterly report.xlsx","is_dir":false}]
fs_free_string(hits);
fs_index_save(h, "docs.fsidx");     // persist it; load it next run
fs_index_free(h);
```

## Search model

- `*` / `?` in the query → glob against the name (`*.docx`, `img_????.jpg`)
- Otherwise → every whitespace token must appear in the name,
  case-insensitive (`annual report` finds `Annual Report 2026.pdf`)
- Deterministic order: by lowercase name, then path

## Design

- **Zero dependencies**, one DLL, plain C ABI, UTF-8 + JSON results,
  thread-local last-error, guarded FFI (valid caller pointers required) — the house pattern shared with
  WinRecovery, so hosts learn one convention for the whole collection.
- **Honest v0.1 scope:** the index is built by walking the filesystem
  (portable `std::fs`; observed symlinks/reparse entries not traversed; unreadable folders
  skipped). It is a snapshot — the NTFS USN-Journal freshness path and
  MFT fast build are v0.2 (see `ROADMAP.md`) and do not change this API.
- **Measured, not marketed.** The first benchmark of the naive matcher
  took 96 ms per query; three fixes (parse the query once, sort the index
  once at build time instead of sorting hits per query, suffix/prefix
  fast paths for `*.ext` / `name*`) brought that to the figures below.

## Explicit search filters

`fs_search_filtered_json(h, "*", "docs", "txt", 1, 50)` finds up to 50
files with final extension `.txt` whose full path contains `docs`.
All filters are case-insensitive and combined before the result limit.
Pass empty strings to disable individual filters; kind 0 = both, 1 =
files, 2 = directories. Empty query still matches nothing. The original
`fs_search_json` syntax and ordering are unchanged.

## Search and loader hardening (October 21, 2026)

- Prefix queries use a binary lower-bound lookup in the sorted index,
  then visit the matching range. Other queries retain scan semantics.
- C ABI searches pin an immutable snapshot and release the global registry
  lock before matching/serialization. Concurrent rebuild/free operations
  do not invalidate an in-flight snapshot; a completed build cannot
  resurrect a freed handle. Concurrent builds use last-completion-wins.
- FSIDX1 loads reject malformed record prefixes, invalid escapes, empty
  paths and NULs, including malformed multibyte prefixes that previously
  could panic. Valid saved indexes retain the same format.
- Release builds allow unwinding so existing FFI panic guards can work.
  Allocation failures and invalid caller pointers are outside that protection.

Reproduce the synthetic benchmark with `cargo run --release --example benchmark`.
The corpus is 200,000 deterministic paths, 200 timed queries after 10 warmups,
limit 50, including JSON output. Measurements on a Linux x86_64 VM (AMD EPYC
9V74, Rust 1.99.1); baseline commit `576097e` versus this implementation:

| Query | Before (µs/query) | After (µs/query) |
|---|---:|---:|
| `item000*` | 10.48 | 11.59 |
| `item100*` | 459.49 | 12.55 |
| `item199*` | 999.22 | 12.10 |
| `zzz*` (no hits) | 1185.52 | 0.28 |
| `*.txt` | 10.41 | 11.17 |
| `199999` (substring) | 6272.82 | 6157.69 |

These are single-process synthetic measurements, not Windows latency promises.
The improvement targets prefix scans; no general substring-speed claim is made.
Snapshot retention can temporarily keep both old and rebuilt indexes in memory.
The optional path filter lowercases candidate paths during matching; profile
that workload before adding a persistent path cache and its memory cost.

## Build & gate

```sh
cargo test                 # matcher, index, persistence, FFI lifecycle
# full gate (fmt, clippy on host + windows targets, tests):
../windevkit/tools/gate.sh
```

## Measured performance

Benchmark: C harness calling the release build, 200 runs per query
(20 for unlimited), on this project's Linux CI VM (the library is
portable; Windows figures will differ — measure on your machine).

| Corpus | Entries | Query | Time |
|---|---|---|---|
| /usr/lib | 53,888 | `*.rs` (limit 50) | 1.13 ms |
| /usr/lib | 53,888 | substring `cargo` | 1.96 ms |
| ~/workspace (dev tree) | 178,323 | `*.rs` (limit 50) | 0.29 ms |
| ~/workspace (dev tree) | 178,323 | `*.rs` (unlimited, ~all hits + JSON) | 5.52 ms |
| ~/workspace (dev tree) | 178,323 | zero-hit glob (full scan) | 3.06 ms |

Index build (one-time walk): 53,888 entries in ~0.5 s; 178,323 in ~4–9 s
on the same VM, varying with cache warmth.

## Verification status

- 15 debug and release tests cover explicit filters, malformed Unicode index input,
  scan/prefix equivalence, snapshot concurrency, freed-handle rebuild rejection,
  and the matcher (glob + token AND), ordering/limits,
  FSIDX1 save/load roundtrip (including escaped paths), walker behavior,
  and the full FFI lifecycle including direct-vs-indexed agreement.
- Windows CI (`windows-latest`) is the link + native-run gate.
- The Windows gate loads the release DLL through Python `ctypes` and runs
  both compiled-library smoke scripts, covering persistence/build behavior,
  snapshot comparisons, and C ABI error handling. See
  [CI run 39577055171](https://github.com/SeanShaffer88/FastSearch/actions/runs/39577055171).
- Performance measured on Linux corpora (above); Windows numbers and
  live use inside a host application are not yet done. USN/MFT paths
  remain v0.2.

## Scan outcome reports

Use `fs_index_build_report_json` to build an index and receive counts of indexed
entries, unreadable folders/entries, and links whose targets were skipped. Its
`partial` flag lets a host explain incomplete coverage. The result is local JSON;
free it with `fs_free_string`. Existing build/search APIs remain compatible.
A build that finishes after its handle was freed fails instead of resurrecting
that handle. See [the API contract](docs/API.md).

## Cancellable builds

`fs_index_build_controlled_report_json` adds synchronous progress checkpoints and
explicit cancellation. Hosts can run it on a worker, show entries examined, and
cancel while preserving the previous index. It does not interrupt blocked OS
calls or final sorting. See the [callback/lifetime contract](docs/API.md).

## Filesystem build bounds

Directory walks use bounded depth-first traversal: up to 2,000,000 entries,
256 MiB of combined path text, and 256 nested directory levels. Limits return
an error and preserve an existing index. Root links/reparse entries are refused.
Unreadable descendants may still be skipped; success does not promise complete
coverage. Rust callers can set `WalkLimits` and inspect skip counts. See
[the API documentation](docs/API.md) for limitations.

## Imported-index limits

The C loader accepts up to 256 MiB of UTF-8 FSIDX1 data and 2,000,000 entries.
Oversized or malformed imports return an error without a partial index handle.
Rust callers can choose other limits through `IndexLoadLimits`. These bounds
apply to imports, not index building/saving or total process memory. See
[the API documentation](docs/API.md) for compatibility and memory details.

## Compare saved snapshots

`fs_index_diff_json(before, after, "*.dll", 200)` compares two handles without
walking the filesystem. It reports matching paths added to the catalog, absent
from it, changed between file and directory, or unchanged. Use this to inspect
what changed between two saved index files, for example after installing an
application or reconnecting an offline drive. Load both FSIDX1 files and keep
the handles alive for the call; the JSON is caller-owned and freed with
`fs_free_string`.

The counts cover every matching path even when detail rows are capped. Blank
query selects all names; other queries use the same glob or AND-token rules as
search. Comparison is exact-path and case-sensitive. An absent path is only a
catalog difference, not evidence that the file was deleted: partial scans,
different roots, and concurrent filesystem changes can create apparent
differences. FSIDX1 does not record root, scan time, or completeness metadata.
See [API details](docs/API.md) for limits and the JSON schema.

## License

MIT.

## Packaged consumer pilot

See the [consumer guide](docs/CONSUMER_GUIDE.md) for the Windows x64 prerelease, C/Python examples, ownership/threading contract and validation limits. Downloads are versioned on the [Releases page](https://github.com/SeanShaffer88/FastSearch/releases).
