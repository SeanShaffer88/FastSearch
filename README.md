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
  thread-local last-error, panic-proof FFI — the house pattern shared with
  WinRecovery, so hosts learn one convention for the whole collection.
- **Honest v0.1 scope:** the index is built by walking the filesystem
  (portable `std::fs`; symlinks never followed; unreadable folders
  skipped). It is a snapshot — the NTFS USN-Journal freshness path and
  MFT fast build are v0.2 (see `ROADMAP.md`) and do not change this API.
- **No invented speed claims.** Nothing here quotes milliseconds that
  were not measured on a named corpus and machine.

## Build & gate

```sh
cargo test                 # matcher, index, persistence, FFI lifecycle
# full gate (fmt, clippy on host + windows targets, tests):
../windevkit/tools/gate.sh
```

## Verification status

- Unit tests cover the matcher (glob + token AND), ordering/limits,
  FSIDX1 save/load roundtrip (including escaped paths), walker behavior,
  and the full FFI lifecycle including direct-vs-indexed agreement.
- Windows CI (`windows-latest`) is the link + native-run gate.
- Not yet done: any performance measurement, USN/MFT paths (v0.2),
  live use inside a host application.

## License

MIT.
