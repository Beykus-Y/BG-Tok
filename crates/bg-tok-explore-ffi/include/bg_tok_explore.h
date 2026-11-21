#ifndef BG_TOK_EXPLORE_H
#define BG_TOK_EXPLORE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Experimental public Explore adapter. Never call blocking decode/read on the
 * UI thread. Host calls must be serialized. Ranges keep their runtime alive;
 * release a range once, after its outstanding reads finish, outside Tokio. */
typedef struct bg_tok_explore bg_tok_explore;
typedef struct bg_tok_explore_range {
    void *context;
    uint64_t length;
    int (*read_at)(void *, uint64_t, uint8_t *, size_t, uint32_t, int (*)(void *), void *);
    void (*release)(void *);
} bg_tok_explore_range;
/* proxy is optional, points to a NUL-terminated host-configured proxy URL. */
bg_tok_explore *bg_tok_explore_create(const char *proxy, uint32_t category);
/* 0 loading, 1 ready, 2 unavailable/error, 3 cancelled. */
int bg_tok_explore_status(const bg_tok_explore *host);
size_t bg_tok_explore_count(const bg_tok_explore *host);
/* field: 0 id, 1 author, 2 description; returns bytes including NUL or 0. */
size_t bg_tok_explore_metadata(const bg_tok_explore *host, size_t index, int field, char *out, size_t capacity);
/* On success transfers a range. Content is NoStore. */
int bg_tok_explore_open(bg_tok_explore *host, size_t index, bg_tok_explore_range *out);
void bg_tok_explore_cancel(bg_tok_explore *host);
void bg_tok_explore_destroy(bg_tok_explore *host);
#ifdef __cplusplus
}
#endif
#endif
