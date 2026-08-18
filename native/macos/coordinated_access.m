#import "coordinated_access.h"

#import <CoreFoundation/CoreFoundation.h>
#import <Foundation/Foundation.h>

#import <stdbool.h>
#import <stdlib.h>
#import <string.h>

static void zwirn_copy_bytes(
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

static void zwirn_copy_string(
    char *destination,
    size_t capacity,
    NSString *source
) {
    zwirn_copy_bytes(destination, capacity, source.UTF8String);
}

static void zwirn_clear_outcome(zwirn_access_outcome *outcome) {
    memset(outcome, 0, sizeof(*outcome));
    outcome->status = ZWIRN_ACCESS_INTERNAL_FAILURE;
}

static bool zwirn_path_matches(
    const char *representation,
    const uint8_t *path,
    size_t path_length
) {
    return representation != NULL &&
        strlen(representation) == path_length &&
        memcmp(representation, path, path_length) == 0;
}

void zwirn_coordinated_access(
    const uint8_t *path,
    size_t path_length,
    int32_t intent,
    zwirn_access_body body,
    void *context,
    zwirn_access_outcome *outcome
) {
    if (outcome == NULL) {
        return;
    }
    zwirn_clear_outcome(outcome);
    if (path == NULL || path_length == 0 || body == NULL ||
        (intent != ZWIRN_ACCESS_READ && intent != ZWIRN_ACCESS_WRITE)) {
        zwirn_copy_bytes(
            outcome->message,
            sizeof(outcome->message),
            "invalid coordinated-access argument"
        );
        return;
    }

    @try {
      @autoreleasepool {
        CFURLRef requested_url = NULL;
        NSFileCoordinator *coordinator = nil;

        @try {
            requested_url = CFURLCreateFromFileSystemRepresentation(
                kCFAllocatorDefault,
                path,
                (CFIndex)path_length,
                false
            );
            if (requested_url == NULL || !zwirn_path_matches(
                    [(NSURL *)requested_url fileSystemRepresentation],
                    path,
                    path_length
                )) {
                outcome->status = ZWIRN_ACCESS_PATH_NOT_REPRESENTABLE;
                zwirn_copy_bytes(
                    outcome->message,
                    sizeof(outcome->message),
                    "Foundation cannot represent the named filesystem path exactly"
                );
                return;
            }

            coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
            NSError *coordination_error = nil;
            void (^accessor)(NSURL *) = ^(NSURL *accessor_url) {
                const char *accessor_path = accessor_url.fileSystemRepresentation;
                if (accessor_path == NULL) {
                    zwirn_copy_bytes(
                        outcome->message,
                        sizeof(outcome->message),
                        "Foundation supplied an accessor URL without a filesystem representation"
                    );
                    return;
                }
                if (!zwirn_path_matches(accessor_path, path, path_length)) {
                    outcome->status = ZWIRN_ACCESSOR_PATH_CHANGED;
                    zwirn_copy_bytes(
                        outcome->message,
                        sizeof(outcome->message),
                        "Foundation supplied a changed accessor path"
                    );
                    return;
                }
                body(context);
                outcome->status = ZWIRN_ACCESS_OK;
            };

            if (intent == ZWIRN_ACCESS_READ) {
                [coordinator coordinateReadingItemAtURL:(NSURL *)requested_url
                                                options:0
                                                  error:&coordination_error
                                             byAccessor:accessor];
            } else {
                [coordinator coordinateWritingItemAtURL:(NSURL *)requested_url
                                                options:0
                                                  error:&coordination_error
                                             byAccessor:accessor];
            }

            if (coordination_error != nil &&
                outcome->status == ZWIRN_ACCESS_INTERNAL_FAILURE) {
                outcome->status = ZWIRN_ACCESS_COORDINATION_FAILED;
                outcome->native_code = coordination_error.code;
                zwirn_copy_string(
                    outcome->native_domain,
                    sizeof(outcome->native_domain),
                    coordination_error.domain
                );
                zwirn_copy_string(
                    outcome->message,
                    sizeof(outcome->message),
                    coordination_error.localizedDescription
                );
            }
        } @catch (NSException *exception) {
            outcome->status = ZWIRN_ACCESS_INTERNAL_FAILURE;
            zwirn_copy_string(
                outcome->native_domain,
                sizeof(outcome->native_domain),
                exception.name
            );
            zwirn_copy_string(
                outcome->message,
                sizeof(outcome->message),
                exception.reason
            );
        } @finally {
            [coordinator release];
            if (requested_url != NULL) {
                CFRelease(requested_url);
            }
        }
      }
    } @catch (...) {
        outcome->status = ZWIRN_ACCESS_INTERNAL_FAILURE;
        zwirn_copy_bytes(
            outcome->native_domain,
            sizeof(outcome->native_domain),
            "NSException"
        );
        zwirn_copy_bytes(
            outcome->message,
            sizeof(outcome->message),
            "native exception escaped coordinated-access cleanup"
        );
    }
}
