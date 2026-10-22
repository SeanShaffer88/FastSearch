/* Build: cl /W4 /WX /I include examples/search.c fast_search.dll.lib */
#include "fastsearch.h"
#include <stdio.h>
int main(int argc, char **argv) {
    uint64_t handle;
    char *report, *hits;
    int status = 0;
    if (argc != 2) { fprintf(stderr, "Pass a UTF-8 test folder.\n"); return 2; }
    handle = fs_index_create();
    if (!handle) return 1;
    report = fs_index_build_report_json(handle, argv[1]);
    if (!report) { fprintf(stderr, "%s\n", fs_last_error()); status = 1; goto done; }
    puts(report); fs_free_string(report);
    hits = fs_search_json(handle, "*.txt", 100);
    if (!hits) { fprintf(stderr, "%s\n", fs_last_error()); status = 1; goto done; }
    puts(hits); fs_free_string(hits);
done:
    if (fs_index_free(handle) != FS_OK) status = 1;
    return status;
}
