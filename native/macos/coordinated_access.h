#ifndef ZWIRN_COORDINATED_ACCESS_H
#define ZWIRN_COORDINATED_ACCESS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Rust consults status only when its callback recorded no completion. */
enum {
    ZWIRN_ACCESS_PATH_NOT_REPRESENTABLE = 1,
    ZWIRN_ACCESS_COORDINATION_FAILED = 2,
    ZWIRN_ACCESSOR_PATH_CHANGED = 3,
    ZWIRN_ACCESS_INTERNAL_FAILURE = 4,
};

enum {
    ZWIRN_ACCESS_READ = 0,
    ZWIRN_ACCESS_WRITE = 1,
};

typedef void (*zwirn_access_body)(void *context);

typedef struct {
    int32_t status;
    int64_t native_code;
    char native_domain[128];
    char message[1024];
} zwirn_access_outcome;

void zwirn_coordinated_access(
    const uint8_t *path,
    size_t path_length,
    int32_t intent,
    zwirn_access_body body,
    void *context,
    zwirn_access_outcome *outcome
);

#ifdef __cplusplus
}
#endif

#endif
