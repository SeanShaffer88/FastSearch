# Release evidence

[v0.1.0-preview.1 evidence](v0.1.0-preview.1.evidence.json) records the existing
unsigned Windows x64 package, exact source commit, checks, runtime scope,
SHA-256/size, notices and remaining gaps. Its source commit intentionally
predates this documentation adoption; no binary was rebuilt or retagged here.

The [WinDevKit v1 contract and verifier](https://github.com/SeanShaffer88/WinDevKit/blob/main/docs/RELEASE_EVIDENCE.md)
provide the shared format. Use a reviewed WinDevKit checkout and the two original
release assets downloaded into a controlled directory:

```powershell
python WinDevKit/tools/release-evidence.py FastSearch/docs/releases/v0.1.0-preview.1.evidence.json --artifacts-dir downloads --release-preflight
```

Review the exact source tag, test claims, package notices and distribution rights
separately. The verifier checks recorded structure and local bytes; it does not
authenticate the publisher or execute the recorded tests.
