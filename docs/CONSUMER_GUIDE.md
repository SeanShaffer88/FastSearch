# Windows x64 consumer preview

The first packaged pilot is `v0.1.0-preview.1`; the library ABI/version functions remain 0.1.0. The ZIP contains the DLL, MSVC import library, C header, C/Python examples, verification scripts, MIT license, API docs and a build manifest identifying the exact source commit. This unsigned prerelease has been tested on Windows 11 Home build 26300 x64. ARM64/x86 and clean Windows 10/runtime acceptance are not claimed.

Use a 64-bit host process. Extract the entire ZIP to a trusted location. Python needs only its standard library; `python examples/search.py fast_search.dll <test-folder>` builds a read-only directory catalog and searches it. For C use the MSVC x64 environment, `cl /W4 /WX /I include examples/search.c fast_search.dll.lib`; place the resulting executable beside the DLL. The package is not an installer and does not register a service or modify indexed files.

## ABI ownership and threading

- Use the included header for this exact build. All input strings are valid readable UTF-8, NUL-terminated; pointer validity remains the caller's responsibility.
- Index handles are opaque `uint64_t`; zero signals failure. Free each successful handle exactly once with `fs_index_free`. Do not reuse a freed handle.
- Caller-owned JSON pointers must use `fs_free_string` from the same DLL. In Python declare their restype as `c_void_p`, not `c_char_p`, so the allocation address is preserved. Static version and thread-local error pointers must never be freed.
- Copy the thread-local error text/code immediately on the calling thread before another library call. Callbacks run synchronously on the calling worker; retain callback/context objects until the call returns and never unwind through C.
- Registry snapshots support concurrent searches. Hosts should coordinate rebuild/free lifetimes, callbacks and DLL unloading; do not unload while calls or owned allocations remain. No asynchronous worker or cancellation guarantee is implied.

## Verification and limits

The native tests cover 32 Rust cases, DLL malformed-Unicode/import and cancellation smoke checks, offline diff semantics, and standalone C/Python consumers against synthetic files. The matching SHA256SUMS and BUILD-INFO identify a package; checksums alone do not authenticate its publisher. Verify the repository/tag and retrieve artifacts through trusted transport.

Default build/import budgets and partial/unreadable/link reporting are in [API.md](API.md). An index is a point-in-time catalog, not a live filesystem guarantee. No engine/model is bundled or required. Rust dependencies are empty; Import-table inspection found VCRUNTIME140.dll and Windows Universal CRT/API-set dependencies. If the runtime is missing, use Microsoft's supported x64 Visual C++ runtime from https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist (not an arbitrary DLL download). No runtime installer is bundled. Clean-host acceptance remains unverified. Existing source performance measurements do not certify every machine.
