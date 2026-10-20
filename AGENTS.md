# FastSearch — agent notes

- Zero dependencies is a design rule (house standard from WinRecovery/WinDevKit). std-only; a new dependency needs a written justification.
- Pure logic (index, matcher, FSIDX1 format) lives in src/lib.rs and must stay testable off-Windows; the walker in src/platform.rs uses std::fs in v0.1.
- v0.1 search semantics are contractual (see header): glob if the query has * or ?, else ANDed case-insensitive substring tokens, matched against the NAME, sorted by (lowercase name, path). Changing them is a breaking API change.
- Gate: use WinDevKit tools/gate.sh (fmt, clippy BOTH targets, full test suite, windows check all-targets). Check/clippy do not link — CI's real build is the link gate.
- Never claim search performance figures that were not measured on a stated corpus/machine.
