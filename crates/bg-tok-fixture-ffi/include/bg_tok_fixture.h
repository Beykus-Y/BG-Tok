#ifndef BG_TOK_FIXTURE_H
#define BG_TOK_FIXTURE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Linux demo/test fixture. Paths are inputs, never passed to BG Media.
 * Each returned range owns its context. Release all ranges and stop all reads
 * before destroying the host. Calls on the host must be serialized. */
typedef struct bg_tok_fixture bg_tok_fixture;
typedef struct bg_tok_fixture_range {
    void *context;
    uint64_t length;
    int (*read_at)(void *, uint64_t, uint8_t *, size_t, uint32_t, int (*)(void *), void *);
    void (*release)(void *);
} bg_tok_fixture_range;
bg_tok_fixture *bg_tok_fixture_create(const char *const *paths, size_t count);
size_t bg_tok_fixture_count(const bg_tok_fixture *host);
uint64_t bg_tok_fixture_read_calls(const bg_tok_fixture *host);
int bg_tok_fixture_open(bg_tok_fixture *host, size_t index, bg_tok_fixture_range *out);
void bg_tok_fixture_cancel(bg_tok_fixture *host);
void bg_tok_fixture_destroy(bg_tok_fixture *host);
#ifdef __cplusplus
}
#endif
#endif
