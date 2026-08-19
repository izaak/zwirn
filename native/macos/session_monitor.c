#include "session_monitor.h"

#include <CoreFoundation/CoreFoundation.h>
#include <CoreServices/CoreServices.h>
#include <dispatch/dispatch.h>

#include <limits.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

struct zwirn_session_monitor {
    FSEventStreamRef stream;
    dispatch_queue_t queue;
    zwirn_monitor_invalidation invalidated;
    void *context;
    bool scheduled;
    bool started;
};

static void zwirn_copy_message(
    char *destination,
    size_t capacity,
    const char *source
) {
    if (destination == NULL || capacity == 0) {
        return;
    }
    if (source == NULL) {
        destination[0] = '\0';
        return;
    }
    size_t length = strnlen(source, capacity - 1);
    memcpy(destination, source, length);
    destination[length] = '\0';
}

static void zwirn_set_outcome(
    zwirn_monitor_outcome *outcome,
    int32_t status,
    const char *message
) {
    memset(outcome, 0, sizeof(*outcome));
    outcome->status = status;
    zwirn_copy_message(outcome->message, sizeof(outcome->message), message);
}

static bool zwirn_valid_absolute_path(const zwirn_monitor_path *path) {
    return path->bytes != NULL && path->length != 0 &&
        path->bytes[0] == '/' &&
        memchr(path->bytes, '\0', path->length) == NULL;
}

/*
 * FSEvents accepts only CFString roots. There is no generally safe substitute
 * for an unrepresentable configured spelling without resolving components or
 * tracking symbolic links, so conversion must succeed for the exact path.
 */
static CFStringRef zwirn_create_watch_path(
    const zwirn_monitor_path *path,
    bool *allocation_failed
) {
    if (path->length == SIZE_MAX) {
        *allocation_failed = true;
        return NULL;
    }
    char *representation = malloc(path->length + 1);
    if (representation == NULL) {
        *allocation_failed = true;
        return NULL;
    }
    memcpy(representation, path->bytes, path->length);
    representation[path->length] = '\0';
    CFStringRef result = CFStringCreateWithFileSystemRepresentation(
        kCFAllocatorDefault,
        representation
    );
    free(representation);
    return result;
}

static void zwirn_fsevents_callback(
    ConstFSEventStreamRef stream,
    void *client_info,
    size_t event_count,
    void *event_paths,
    const FSEventStreamEventFlags event_flags[],
    const FSEventStreamEventId event_ids[]
) {
    (void)stream;
    (void)event_paths;
    (void)event_flags;
    (void)event_ids;

    zwirn_session_monitor *monitor = client_info;
    if (event_count != 0 && monitor != NULL && monitor->invalidated != NULL) {
        /* Every batch attempts to mark the session dirty; none is filtered. */
        monitor->invalidated(monitor->context);
    }
}

static void zwirn_dispatch_noop(void *context) {
    (void)context;
}

static void zwirn_destroy_monitor(zwirn_session_monitor *monitor) {
    if (monitor == NULL) {
        return;
    }
    if (monitor->stream != NULL && monitor->started) {
        FSEventStreamStop(monitor->stream);
    }
    if (monitor->stream != NULL && monitor->scheduled) {
        FSEventStreamInvalidate(monitor->stream);
        dispatch_sync_f(monitor->queue, NULL, zwirn_dispatch_noop);
    }
    if (monitor->stream != NULL) {
        FSEventStreamRelease(monitor->stream);
    }
    if (monitor->queue != NULL) {
        dispatch_release(monitor->queue);
    }
    free(monitor);
}

zwirn_session_monitor *zwirn_session_monitor_start(
    const zwirn_monitor_path *paths,
    size_t path_count,
    zwirn_monitor_invalidation invalidated,
    void *context,
    zwirn_monitor_outcome *outcome
) {
    if (outcome == NULL) {
        return NULL;
    }
    zwirn_set_outcome(
        outcome,
        ZWIRN_MONITOR_INVALID_ARGUMENT,
        "invalid session-monitor argument"
    );
    if (paths == NULL || path_count == 0 ||
        path_count > (size_t)LONG_MAX || invalidated == NULL) {
        return NULL;
    }
    for (size_t index = 0; index < path_count; index++) {
        if (!zwirn_valid_absolute_path(&paths[index])) {
            return NULL;
        }
    }

    zwirn_session_monitor *monitor = calloc(1, sizeof(*monitor));
    if (monitor == NULL) {
        zwirn_set_outcome(
            outcome,
            ZWIRN_MONITOR_ALLOCATION_FAILED,
            "cannot allocate the session monitor"
        );
        return NULL;
    }
    monitor->invalidated = invalidated;
    monitor->context = context;

    CFMutableArrayRef watched_paths = CFArrayCreateMutable(
        kCFAllocatorDefault,
        (CFIndex)path_count,
        &kCFTypeArrayCallBacks
    );
    if (watched_paths == NULL) {
        zwirn_set_outcome(
            outcome,
            ZWIRN_MONITOR_ALLOCATION_FAILED,
            "cannot allocate the watched-path array"
        );
        zwirn_destroy_monitor(monitor);
        return NULL;
    }

    for (size_t index = 0; index < path_count; index++) {
        bool allocation_failed = false;
        CFStringRef watched_path = zwirn_create_watch_path(
            &paths[index],
            &allocation_failed
        );
        if (watched_path == NULL) {
            CFRelease(watched_paths);
            zwirn_set_outcome(
                outcome,
                allocation_failed
                    ? ZWIRN_MONITOR_ALLOCATION_FAILED
                    : ZWIRN_MONITOR_PATH_NOT_REPRESENTABLE,
                allocation_failed
                    ? "cannot allocate a watched path"
                    : "a configured watch path is not representable by Core Foundation"
            );
            zwirn_destroy_monitor(monitor);
            return NULL;
        }
        CFRange all_paths = CFRangeMake(0, CFArrayGetCount(watched_paths));
        if (!CFArrayContainsValue(watched_paths, all_paths, watched_path)) {
            CFArrayAppendValue(watched_paths, watched_path);
        }
        CFRelease(watched_path);
    }

    FSEventStreamContext stream_context = {
        .version = 0,
        .info = monitor,
        .retain = NULL,
        .release = NULL,
        .copyDescription = NULL,
    };
    FSEventStreamCreateFlags flags =
        kFSEventStreamCreateFlagNoDefer |
        kFSEventStreamCreateFlagWatchRoot |
        kFSEventStreamCreateFlagFileEvents |
        kFSEventStreamCreateFlagMarkSelf;
    monitor->stream = FSEventStreamCreate(
        kCFAllocatorDefault,
        zwirn_fsevents_callback,
        &stream_context,
        watched_paths,
        kFSEventStreamEventIdSinceNow,
        0.05,
        flags
    );
    CFRelease(watched_paths);
    if (monitor->stream == NULL) {
        zwirn_set_outcome(
            outcome,
            ZWIRN_MONITOR_STREAM_CREATE_FAILED,
            "FSEventStreamCreate failed"
        );
        zwirn_destroy_monitor(monitor);
        return NULL;
    }

    monitor->queue = dispatch_queue_create(
        "org.zwirn.session-monitor.fsevents",
        DISPATCH_QUEUE_SERIAL
    );
    if (monitor->queue == NULL) {
        zwirn_set_outcome(
            outcome,
            ZWIRN_MONITOR_QUEUE_CREATE_FAILED,
            "cannot create the FSEvents delivery queue"
        );
        zwirn_destroy_monitor(monitor);
        return NULL;
    }

    FSEventStreamSetDispatchQueue(monitor->stream, monitor->queue);
    monitor->scheduled = true;
    if (!FSEventStreamStart(monitor->stream)) {
        zwirn_set_outcome(
            outcome,
            ZWIRN_MONITOR_STREAM_START_FAILED,
            "FSEventStreamStart failed"
        );
        zwirn_destroy_monitor(monitor);
        return NULL;
    }
    monitor->started = true;
    zwirn_set_outcome(outcome, ZWIRN_MONITOR_OK, "");
    return monitor;
}

void zwirn_session_monitor_stop(zwirn_session_monitor *monitor) {
    zwirn_destroy_monitor(monitor);
}

void zwirn_session_monitor_flush(zwirn_session_monitor *monitor) {
    if (monitor != NULL && monitor->stream != NULL && monitor->started) {
        FSEventStreamFlushSync(monitor->stream);
    }
}
