/*
 * FastSearch.dll — C ABI
 * ======================
 * Build an index of a directory tree once, then search it in-memory.
 * A one-shot direct search (walk + search, nothing kept) is included as
 * a convenience and as a correctness baseline for the indexed path.
 *
 * Conventions
 * -----------
 * - All strings are UTF-8, NUL-terminated.
 * - Index handles are opaque uint64_t values from fs_index_create() or
 *   fs_index_load(); 0 is never valid. Release with fs_index_free().
 *   Handles are thread-safe.
 * - Functions returning char* transfer ownership; free with
 *   fs_free_string(). On failure they return NULL — see fs_last_error().
 * - Integer functions return FS_OK (0) or an FS_ERR_* code.
 *
 * Search model (v0.1)
 * -------------------
 * - Query containing * or ?  -> glob match against the file NAME.
 * - Otherwise: whitespace-separated tokens, ALL must appear in the name
 *   (case-insensitive substring, AND semantics).
 * - Results sort by (lowercased name, path). max_results <= 0 = no limit.
 *
 * v0.1 indexes by walking the filesystem (std). The NTFS USN-Journal /
 * MFT fast path is planned for v0.2 — the API does not change.
 */

#ifndef FASTSEARCH_H
#define FASTSEARCH_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32) && defined(FS_BUILD)
#define FS_API __declspec(dllexport)
#elif defined(_WIN32)
#define FS_API __declspec(dllimport)
#else
#define FS_API
#endif

#define FS_OK                   0
#define FS_ERR_INVALID_ARG      1
#define FS_ERR_NOT_SUPPORTED    2
#define FS_ERR_NOT_FOUND        3
#define FS_ERR_ACCESS_DENIED    4
#define FS_ERR_PLATFORM         5
#define FS_ERR_INTERNAL         6

/* Version & errors ------------------------------------------------------- */
FS_API const char *fs_version(void);          /* static string; do NOT free */
FS_API uint32_t fs_version_major(void);
FS_API uint32_t fs_version_minor(void);
FS_API uint32_t fs_version_patch(void);
FS_API const char *fs_last_error(void);       /* thread-local; do NOT free */
FS_API int fs_last_error_code(void);
FS_API void fs_free_string(char *p);          /* frees any *_json result */

/* Index lifecycle ---------------------------------------------------------- */
FS_API uint64_t fs_index_create(void);
FS_API int fs_index_free(uint64_t handle);
/* Entry count, or -1 on error. */
FS_API int64_t fs_index_count(uint64_t handle);
/* Walk root and REPLACE the index contents. Unreadable subdirectories are
 * skipped; an unusable root is an error. */
FS_API int fs_index_build(uint64_t handle, const char *root_utf8);

/* Search --------------------------------------------------------------------- */
/* JSON array: [{"path":"...","name":"...","is_dir":false}, ...] */
FS_API char *fs_search_json(uint64_t handle, const char *query_utf8, int max_results);
/* Explicit filters (ANDed before max_results). Non-null UTF-8 strings;
 * empty path/extension disables that filter. Path = case-insensitive full
 * path substring; extension = final extension without a dot, insensitive
 * to case, files only (.txt alone has no extension). kind: 0 both, 1 files,
 * 2 directories. Use query "*" to match all names before filtering. */
FS_API char *fs_search_filtered_json(uint64_t handle, const char *query_utf8,
    const char *path_contains_utf8, const char *extension_utf8,
    int kind, int max_results);
/* Walk + search in one call; keeps no index. */
FS_API char *fs_search_direct_json(const char *root_utf8, const char *query_utf8, int max_results);

/* Persistence (FSIDX1 text format) -------------------------------------------- */
FS_API int fs_index_save(uint64_t handle, const char *path_utf8);
FS_API uint64_t fs_index_load(const char *path_utf8);   /* 0 on error */

#ifdef __cplusplus
}
#endif

#endif /* FASTSEARCH_H */
