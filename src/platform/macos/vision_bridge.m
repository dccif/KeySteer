#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>

#include <stdlib.h>
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
};

static NmkVisionResult *resultWithStatus(int32_t status, NSString *message) {
    NmkVisionResult *result = calloc(1, sizeof(NmkVisionResult));
    if (result == NULL) return NULL;
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

    dispatch_group_t group = dispatch_group_create();
    __block CGImageRef captured = NULL;
    __block NSError *captureError = nil;
    dispatch_group_enter(group);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error != nil || content.displays.count == 0) {
            captureError = error;
            dispatch_group_leave(group);
            return;
        }
        SCDisplay *display = displayForID(displayID, content);
        if (display == nil) {
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
            dispatch_group_leave(group);
        }];
    }];

    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)timeoutMS * NSEC_PER_MSEC);
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

NmkVisionResult *NmkDetectVisionElements(CGRect windowBounds, NmkVisionConfig config) {
    @autoreleasepool {
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
        if (!performed || error != nil) {
            return resultWithStatus(NMK_VISION_FAILED, error.localizedDescription ?: @"Vision request failed");
        }

        NSMutableArray<NSDictionary *> *regions = [NSMutableArray array];
        for (VNRectangleObservation *observation in rectangleRequest.results ?: @[]) {
            if (observation.confidence < config.minimum_confidence) continue;
            CGRect box = observation.boundingBox;
            [regions addObject:@{@"x": @(box.origin.x), @"y": @(box.origin.y),
                @"w": @(box.size.width), @"h": @(box.size.height),
                @"confidence": @(observation.confidence), @"text": @NO, @"label": @""}];
        }
        for (VNRecognizedTextObservation *observation in textRequest.results ?: @[]) {
            if (observation.confidence < config.minimum_confidence) continue;
            VNRecognizedText *candidate = [observation topCandidates:1].firstObject;
            CGRect box = observation.boundingBox;
            [regions addObject:@{@"x": @(box.origin.x), @"y": @(box.origin.y),
                @"w": @(box.size.width), @"h": @(box.size.height),
                @"confidence": @(observation.confidence), @"text": @YES,
                @"label": candidate.string ?: @""}];
        }

        NmkVisionResult *result = resultWithStatus(NMK_VISION_OK, nil);
        if (result == NULL) return NULL;
        result->captured_bounds = capturedBounds;
        result->count = regions.count;
        result->regions = calloc(result->count, sizeof(NmkVisionRegion));
        if (result->count > 0 && result->regions == NULL) {
            free(result);
            return NULL;
        }
        for (NSUInteger index = 0; index < regions.count; index++) {
            NSDictionary *region = regions[index];
            NmkVisionRegion *out = &result->regions[index];
            out->x = [region[@"x"] doubleValue];
            out->y = [region[@"y"] doubleValue];
            out->width = [region[@"w"] doubleValue];
            out->height = [region[@"h"] doubleValue];
            out->confidence = [region[@"confidence"] doubleValue];
            out->is_text = [region[@"text"] boolValue];
            out->label = strdup([region[@"label"] UTF8String]);
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
