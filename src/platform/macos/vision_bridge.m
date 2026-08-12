#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>

#include <stdlib.h>
#include <stdatomic.h>
#include <string.h>
#include <unistd.h>

typedef struct {
    bool detect_text;
    bool detect_rectangles;
    uint64_t timeout_ms;
    double minimum_confidence;
    uint64_t rectangle_max_candidates;
    double rectangle_min_size;
    double rectangle_min_aspect;
    double rectangle_max_aspect;
} NmkVisionConfig;

typedef struct {
    double x;
    double y;
    double width;
    double height;
    double confidence;
    bool is_text;
    char *label;
} NmkVisionRegion;

typedef struct {
    uint32_t abi_version;
    uint32_t region_stride;
    int32_t status;
    NmkVisionRegion *regions;
    uint64_t count;
    char *message;
    CGRect captured_bounds;
} NmkVisionResult;

enum {
    NMK_VISION_OK = 0,
    NMK_VISION_PERMISSION = 1,
    NMK_VISION_TIMEOUT = 2,
    NMK_VISION_FAILED = 3,
    NMK_VISION_CONTEXT_CHANGED = 4,
};

enum { NMK_VISION_ABI_VERSION = 1, NMK_MAX_VISION_REGIONS = 2000 };

static _Atomic uint64_t latestVisionScan = 0;
static _Atomic bool captureInFlight = false;

void NmkSetLatestVisionScan(uint64_t scanID) {
    atomic_store_explicit(&latestVisionScan, scanID, memory_order_release);
}

static bool scanIsCurrent(uint64_t scanID) {
    return atomic_load_explicit(&latestVisionScan, memory_order_acquire) == scanID;
}

static bool tryAcquireCapture(void) {
    bool expected = false;
    return atomic_compare_exchange_strong_explicit(
        &captureInFlight,
        &expected,
        true,
        memory_order_acq_rel,
        memory_order_acquire);
}

static void releaseCapture(void) {
    atomic_store_explicit(&captureInFlight, false, memory_order_release);
}

static NmkVisionResult *resultWithStatus(int32_t status, NSString *message) {
    NmkVisionResult *result = calloc(1, sizeof(NmkVisionResult));
    if (result == NULL) return NULL;
    result->abi_version = NMK_VISION_ABI_VERSION;
    result->region_stride = sizeof(NmkVisionRegion);
    result->status = status;
    if (message.length > 0) result->message = strdup(message.UTF8String);
    return result;
}

static SCDisplay *displayForID(CGDirectDisplayID displayID, SCShareableContent *content) {
    for (SCDisplay *display in content.displays) {
        if (display.displayID == displayID) return display;
    }
    return content.displays.firstObject;
}

static CGImageRef captureRegion(
    CGRect region,
    uint64_t timeoutMS,
    int32_t *status,
    NSString **message,
    CGRect *capturedBounds) {
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetDisplaysWithRect(region, 32, ids, &count) != kCGErrorSuccess || count == 0) {
        *status = NMK_VISION_FAILED;
        *message = @"No display contains the focused window";
        return NULL;
    }
    CGDirectDisplayID displayID = ids[0];
    CGRect displayBounds = CGDisplayBounds(displayID);
    CGRect clipped = CGRectIntersection(region, displayBounds);
    if (CGRectIsNull(clipped) || CGRectIsEmpty(clipped)) {
        *status = NMK_VISION_FAILED;
        *message = @"The focused window is outside the captured display";
        return NULL;
    }
    if (capturedBounds != NULL) *capturedBounds = clipped;

    // ScreenCaptureKit has no cancellation API. Keep this permit until the
    // actual completion callback, not merely until the synchronous caller
    // times out, so full-resolution captures can never overlap.
    if (!tryAcquireCapture()) {
        *status = NMK_VISION_TIMEOUT;
        *message = @"A previous screen capture is still completing";
        return NULL;
    }

    dispatch_group_t group = dispatch_group_create();
    __block CGImageRef captured = NULL;
    __block NSError *captureError = nil;
    dispatch_group_enter(group);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error != nil || content.displays.count == 0) {
            captureError = error;
            releaseCapture();
            dispatch_group_leave(group);
            return;
        }
        SCDisplay *display = displayForID(displayID, content);
        if (display == nil) {
            releaseCapture();
            dispatch_group_leave(group);
            return;
        }
        NSMutableArray<SCWindow *> *excludedWindows = [NSMutableArray array];
        pid_t ownPID = getpid();
        for (SCWindow *window in content.windows) {
            if (window.owningApplication.processID == ownPID) {
                [excludedWindows addObject:window];
            }
        }
        SCContentFilter *filter = [[SCContentFilter alloc]
            initWithDisplay:display excludingWindows:excludedWindows];
        SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
        CGFloat scaleX = (CGFloat)CGDisplayPixelsWide(displayID) / displayBounds.size.width;
        CGFloat scaleY = (CGFloat)CGDisplayPixelsHigh(displayID) / displayBounds.size.height;
        configuration.sourceRect = CGRectMake(
            clipped.origin.x - displayBounds.origin.x,
            clipped.origin.y - displayBounds.origin.y,
            clipped.size.width,
            clipped.size.height);
        configuration.width = MAX(1, (size_t)llround(clipped.size.width * scaleX));
        configuration.height = MAX(1, (size_t)llround(clipped.size.height * scaleY));
        configuration.showsCursor = NO;
        [SCScreenshotManager captureImageWithFilter:filter configuration:configuration completionHandler:^(CGImageRef image, NSError *error) {
            captureError = error;
            if (image != NULL) captured = CGImageRetain(image);
            releaseCapture();
            dispatch_group_leave(group);
        }];
    }];

    uint64_t boundedTimeoutMS = MIN(MAX(timeoutMS, 1), 30000);
    dispatch_time_t deadline = dispatch_time(
        DISPATCH_TIME_NOW,
        (int64_t)boundedTimeoutMS * NSEC_PER_MSEC);
    if (dispatch_group_wait(group, deadline) != 0) {
        // The ScreenCaptureKit operation cannot be cancelled. Release a late
        // image after its completion block leaves the group instead of leaking
        // the retained CGImage when the caller has already timed out.
        dispatch_group_notify(group, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
            if (captured != NULL) CGImageRelease(captured);
        });
        *status = NMK_VISION_TIMEOUT;
        *message = @"Screen capture timed out";
        return NULL;
    }
    if (captured == NULL) {
        *status = NMK_VISION_FAILED;
        *message = captureError.localizedDescription ?: @"ScreenCaptureKit did not return an image";
    }
    return captured;
}

void NmkFreeVisionResult(NmkVisionResult *result);

NmkVisionResult *NmkDetectVisionElements(
    CGRect windowBounds,
    NmkVisionConfig config,
    uint64_t scanID) {
    @autoreleasepool {
        config.timeout_ms = MIN(MAX(config.timeout_ms, 1), 30000);
        config.rectangle_max_candidates = MIN(
            MAX(config.rectangle_max_candidates, 1),
            (uint64_t)NMK_MAX_VISION_REGIONS);
        if (!scanIsCurrent(scanID)) {
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        if (!CGPreflightScreenCaptureAccess()) {
            static dispatch_once_t requestOnce;
            static bool granted = false;
            dispatch_once(&requestOnce, ^{ granted = CGRequestScreenCaptureAccess(); });
            if (!granted && !CGPreflightScreenCaptureAccess()) {
                return resultWithStatus(NMK_VISION_PERMISSION, @"Screen Recording permission is required for Vision hints");
            }
        }

        int32_t captureStatus = NMK_VISION_OK;
        NSString *captureMessage = nil;
        CGRect capturedBounds = CGRectZero;
        CGImageRef image = captureRegion(
            windowBounds,
            config.timeout_ms,
            &captureStatus,
            &captureMessage,
            &capturedBounds);
        if (image == NULL) return resultWithStatus(captureStatus, captureMessage);
        if (!scanIsCurrent(scanID)) {
            CGImageRelease(image);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }

        NSMutableArray<VNRequest *> *requests = [NSMutableArray array];
        VNDetectRectanglesRequest *rectangleRequest = nil;
        VNRecognizeTextRequest *textRequest = nil;
        if (config.detect_rectangles) {
            rectangleRequest = [[VNDetectRectanglesRequest alloc] init];
            rectangleRequest.maximumObservations = (NSUInteger)config.rectangle_max_candidates;
            rectangleRequest.minimumSize = (float)config.rectangle_min_size;
            rectangleRequest.minimumAspectRatio = (float)config.rectangle_min_aspect;
            rectangleRequest.maximumAspectRatio = (float)config.rectangle_max_aspect;
            [requests addObject:rectangleRequest];
        }
        if (config.detect_text) {
            textRequest = [[VNRecognizeTextRequest alloc] init];
            textRequest.recognitionLevel = VNRequestTextRecognitionLevelFast;
            textRequest.usesLanguageCorrection = NO;
            [requests addObject:textRequest];
        }

        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
        NSError *error = nil;
        BOOL performed = [handler performRequests:requests error:&error];
        CGImageRelease(image);
        if (!scanIsCurrent(scanID)) {
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        if (!performed || error != nil) {
            return resultWithStatus(NMK_VISION_FAILED, error.localizedDescription ?: @"Vision request failed");
        }

        NSArray<VNRectangleObservation *> *rectangleResults = rectangleRequest.results ?: @[];
        NSArray<VNRecognizedTextObservation *> *textResults = textRequest.results ?: @[];
        NSUInteger capacity = MIN(
            (NSUInteger)NMK_MAX_VISION_REGIONS,
            rectangleResults.count + textResults.count);
        NmkVisionResult *result = resultWithStatus(NMK_VISION_OK, nil);
        if (result == NULL) return NULL;
        result->captured_bounds = capturedBounds;
        result->regions = calloc(capacity, sizeof(NmkVisionRegion));
        if (capacity > 0 && result->regions == NULL) {
            free(result);
            return NULL;
        }

        for (VNRecognizedTextObservation *observation in textResults) {
            if (result->count >= capacity) break;
            if (observation.confidence < config.minimum_confidence) continue;
            VNRecognizedText *candidate = [observation topCandidates:1].firstObject;
            CGRect box = observation.boundingBox;
            NmkVisionRegion *out = &result->regions[result->count++];
            out->x = box.origin.x;
            out->y = box.origin.y;
            out->width = box.size.width;
            out->height = box.size.height;
            out->confidence = observation.confidence;
            out->is_text = true;
            out->label = strdup([(candidate.string ?: @"") UTF8String]);
        }
        for (VNRectangleObservation *observation in rectangleResults) {
            if (result->count >= capacity) break;
            if (observation.confidence < config.minimum_confidence) continue;
            CGRect box = observation.boundingBox;
            NmkVisionRegion *out = &result->regions[result->count++];
            out->x = box.origin.x;
            out->y = box.origin.y;
            out->width = box.size.width;
            out->height = box.size.height;
            out->confidence = observation.confidence;
            out->is_text = false;
            out->label = strdup("");
        }
        if (!scanIsCurrent(scanID)) {
            NmkFreeVisionResult(result);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        return result;
    }
}

void NmkFreeVisionResult(NmkVisionResult *result) {
    if (result == NULL) return;
    for (uint64_t index = 0; index < result->count; index++) free(result->regions[index].label);
    free(result->regions);
    free(result->message);
    free(result);
}
