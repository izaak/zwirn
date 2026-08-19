#ifndef ZWIRN_SESSION_MONITOR_H
#define ZWIRN_SESSION_MONITOR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    ZWIRN_MONITOR_OK = 0,
    ZWIRN_MONITOR_INVALID_ARGUMENT = 1,
    ZWIRN_MONITOR_ALLOCATION_FAILED = 2,
    ZWIRN_MONITOR_PATH_NOT_REPRESENTABLE = 3,
    ZWIRN_MONITOR_STREAM_CREATE_FAILED = 4,
    ZWIRN_MONITOR_QUEUE_CREATE_FAILED = 5,
    ZWIRN_MONITOR_STREAM_START_FAILED = 6,
};

typedef struct {
    const uint8_t *bytes;
    size_t length;
} zwirn_monitor_path;

typedef struct {
    int32_t status;
    char message[1024];
} zwirn_monitor_outcome;

typedef void (*zwirn_monitor_invalidation)(void *context);

typedef struct zwirn_session_monitor zwirn_session_monitor;

zwirn_session_monitor *zwirn_session_monitor_start(
    const zwirn_monitor_path *paths,
    size_t path_count,
    zwirn_monitor_invalidation invalidated,
    void *context,
    zwirn_monitor_outcome *outcome
);

void zwirn_session_monitor_stop(zwirn_session_monitor *monitor);

void zwirn_session_monitor_flush(zwirn_session_monitor *monitor);

#ifdef __cplusplus
}
#endif

#endif
