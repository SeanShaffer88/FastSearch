/* FastSearch C example */
#include "fastsearch.h"
#include <stdio.h>
int main(int argc, char **argv) {
    const char *root = argc > 1 ? argv[1] : ".";
    const char *query = argc > 2 ? argv[2] : "*.txt";
    uint64_t h = fs_index_create();
    if (fs_index_build(h, root) != FS_OK) {
        fprintf(stderr, "build failed (%d): %s\n", fs_last_error_code(), fs_last_error());
        return 1;
    }
    printf("indexed %lld entries under %s\n", (long long)fs_index_count(h), root);
    char *json = fs_search_json(h, query, 20);
    if (json) { printf("%s\n", json); fs_free_string(json); }
    fs_index_free(h);
    return 0;
}
