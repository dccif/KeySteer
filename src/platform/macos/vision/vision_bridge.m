#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>

#include <stdlib.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <math.h>
#include <unistd.h>
#include <pthread.h>
#include <sys/sysctl.h>
#include "point_capture.h"

uint32_t NmkVisionOcrPerformanceCores(void) {
    int32_t cores = 0;
    size_t size = sizeof(cores);
    // Apple Silicon lists performance cores separately from efficiency cores.
    // Intel Macs fall back to physical cores, without counting SMT twice.
    if (sysctlbyname("hw.perflevel0.physicalcpu", &cores, &size, NULL, 0) == 0 && cores > 0) {
        return (uint32_t)cores;
    }
    size = sizeof(cores);
    if (sysctlbyname("hw.physicalcpu", &cores, &size, NULL, 0) == 0 && cores > 0) {
        return (uint32_t)cores;
    }
    return 0;
}

typedef struct {
    bool detect_text;
    bool detect_rectangles;
    bool detect_contours;
    uint64_t timeout_ms;
    double minimum_confidence;
    uint64_t rectangle_max_candidates;
    double rectangle_min_size;
    double rectangle_min_aspect;
    double rectangle_max_aspect;
    uint64_t maximum_capture_pixels;
    uint64_t maximum_capture_dimension;
} NmkVisionConfig;

typedef struct {
    double x;
    double y;
    double width;
    double height;
    double confidence;
    bool is_text;
    char *label;
    uint64_t label_len;
} NmkVisionRegion;

typedef struct {
    uint32_t abi_version;
    uint32_t result_size;
    uint32_t region_stride;
    int32_t status;
    NmkVisionRegion *regions;
    uint64_t count;
    char *message;
    uint64_t message_len;
    CGRect captured_bounds;
    CGImageRef image;
    uint8_t *gray;
    uint64_t gray_width;
    uint64_t gray_height;
    CFDataRef pixel_owner;
    uint64_t pixel_length;
    uint64_t pixel_stride;
} NmkVisionResult;

enum {
    NMK_VISION_OK = 0,
    NMK_VISION_PERMISSION = 1,
    NMK_VISION_TIMEOUT = 2,
    NMK_VISION_FAILED = 3,
    NMK_VISION_CONTEXT_CHANGED = 4,
};

enum { NMK_VISION_ABI_VERSION = 4 };
// Matches the fixed OCR lanes in scan_workers.rs.
enum { NMK_MAX_OCR_IN_FLIGHT = 16 };

static _Atomic uint64_t latestVisionScan = 0;
static _Atomic bool captureInFlight = false;
// Keep every in-flight tile cancellable. Strong references exist only while
// performRequests is running; no request or image is cached between scans.
static pthread_mutex_t visionRequestLock = PTHREAD_MUTEX_INITIALIZER;
static VNRequest *activeRectangleRequest = nil;
static VNRequest *activeTextRequests[NMK_MAX_OCR_IN_FLIGHT] = {nil};
static uint64_t activeRectangleScan = 0;
static uint64_t activeTextScans[NMK_MAX_OCR_IN_FLIGHT] = {0};
static dispatch_semaphore_t activeCaptureWake = nil;
static uint64_t activeCaptureScan = 0;
static dispatch_semaphore_t capturePermitWake = nil;

static void cancelVisionRequests(uint64_t scanID, bool superseded) {
    @autoreleasepool {
        VNRequest *rectangle = nil;
        VNRequest *texts[NMK_MAX_OCR_IN_FLIGHT] = {nil};
        dispatch_semaphore_t captureWake = nil;
        pthread_mutex_lock(&visionRequestLock);
        if (superseded) {
            atomic_store_explicit(&latestVisionScan, scanID, memory_order_release);
        } else {
            // A deadline also invalidates native result conversion, but a late
            // cancellation must never invalidate a newer scan's generation.
            uint64_t expected = scanID;
            atomic_compare_exchange_strong_explicit(&latestVisionScan, &expected, 0,
                memory_order_acq_rel, memory_order_acquire);
        }
        if (superseded ? activeRectangleScan != scanID : activeRectangleScan == scanID) {
            rectangle = activeRectangleRequest;
        }
        for (size_t index = 0; index < NMK_MAX_OCR_IN_FLIGHT; index++) {
            if (superseded ? activeTextScans[index] != scanID : activeTextScans[index] == scanID) {
                texts[index] = activeTextRequests[index];
            }
        }
        if (superseded ? activeCaptureScan != scanID : activeCaptureScan == scanID) {
            captureWake = activeCaptureWake;
        }
        pthread_mutex_unlock(&visionRequestLock);
        // Never call into Vision under our registration lock or wait for a worker.
        if (captureWake != nil) dispatch_semaphore_signal(captureWake);
        [rectangle cancel];
        for (size_t index = 0; index < NMK_MAX_OCR_IN_FLIGHT; index++) [texts[index] cancel];
    }
}

void NmkSetLatestVisionScan(uint64_t scanID) { cancelVisionRequests(scanID, true); }
void NmkCancelVisionScan(uint64_t scanID) { cancelVisionRequests(scanID, false); }

static bool scanIsCurrent(uint64_t scanID) {
    return atomic_load_explicit(&latestVisionScan, memory_order_acquire) == scanID;
}

static bool registerVisionRequests(VNRequest *rectangle, VNRequest *text, uint64_t scanID) {
    pthread_mutex_lock(&visionRequestLock);
    bool current = scanIsCurrent(scanID);
    size_t textSlot = 0;
    if (text != nil) {
        while (textSlot < NMK_MAX_OCR_IN_FLIGHT && activeTextRequests[textSlot] != nil) textSlot++;
        current = current && textSlot < NMK_MAX_OCR_IN_FLIGHT;
    }
    if (current) {
        if (rectangle != nil) {
            activeRectangleRequest = rectangle;
            activeRectangleScan = scanID;
        }
        if (text != nil) {
            activeTextRequests[textSlot] = text;
            activeTextScans[textSlot] = scanID;
        }
    }
    pthread_mutex_unlock(&visionRequestLock);
    return current;
}

static void unregisterVisionRequests(VNRequest *rectangle, VNRequest *text) {
    pthread_mutex_lock(&visionRequestLock);
    if (rectangle != nil && activeRectangleRequest == rectangle) activeRectangleRequest = nil;
    if (text != nil) {
        for (size_t index = 0; index < NMK_MAX_OCR_IN_FLIGHT; index++) {
            if (activeTextRequests[index] == text) activeTextRequests[index] = nil;
        }
    }
    pthread_mutex_unlock(&visionRequestLock);
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
    pthread_mutex_lock(&visionRequestLock);
    dispatch_semaphore_t wake = capturePermitWake;
    pthread_mutex_unlock(&visionRequestLock);
    if (wake != nil) dispatch_semaphore_signal(wake);
}

// A cancelled screenshot still owns its permit until ScreenCaptureKit returns.
// A new scan waits for that completion instead of losing its whole visual scan.
// This wake is separate from the new screenshot's completion semaphore.
static int32_t acquireScanCapture(uint64_t scanID, dispatch_time_t deadline) {
    dispatch_semaphore_t wake = dispatch_semaphore_create(0);
    pthread_mutex_lock(&visionRequestLock);
    bool current = scanIsCurrent(scanID);
    if (current) {
        activeCaptureWake = wake;
        activeCaptureScan = scanID;
        capturePermitWake = wake;
    }
    pthread_mutex_unlock(&visionRequestLock);
    if (!current) return NMK_VISION_CONTEXT_CHANGED;

    bool acquired = false;
    int32_t status = NMK_VISION_OK;
    while (scanIsCurrent(scanID) && !(acquired = tryAcquireCapture())) {
        if (dispatch_semaphore_wait(wake, deadline) != 0) {
            status = NMK_VISION_TIMEOUT;
            break;
        }
    }
    pthread_mutex_lock(&visionRequestLock);
    if (capturePermitWake == wake) capturePermitWake = nil;
    if (activeCaptureWake == wake) activeCaptureWake = nil;
    pthread_mutex_unlock(&visionRequestLock);
    if (!scanIsCurrent(scanID)) status = NMK_VISION_CONTEXT_CHANGED;
    if (acquired && status != NMK_VISION_OK) releaseCapture();
    return status;
}

static NmkVisionResult *resultWithStatus(int32_t status, NSString *message) {
    NmkVisionResult *result = calloc(1, sizeof(NmkVisionResult));
    if (result == NULL) return NULL;
    result->abi_version = NMK_VISION_ABI_VERSION;
    result->result_size = sizeof(NmkVisionResult);
    result->region_stride = sizeof(NmkVisionRegion);
    result->status = status;
    if (message.length > 0) {
        const char *utf8 = message.UTF8String;
        if (utf8 != NULL) {
            result->message_len = (uint64_t)strlen(utf8);
            result->message = strdup(utf8);
        }
    }
    return result;
}

// Backend scan-thread cache. Filters contain no screenshot pixels or OCR
// models, and exclude the whole owning application so reused filters also
// exclude overlay windows created after the first capture.
@interface NmkCaptureFilter : NSObject
@property(nonatomic, strong) SCContentFilter *filter;
@property(nonatomic) CGRect bounds;
@property(nonatomic) size_t pixelWidth;
@property(nonatomic) size_t pixelHeight;
@end
@implementation NmkCaptureFilter
@end

@interface NmkCaptureContext : NSObject
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NmkCaptureFilter *> *filters;
@end
@implementation NmkCaptureContext
@end

uintptr_t NmkCreateVisionCaptureContext(void) {
    @autoreleasepool {
        NmkCaptureContext *context = [[NmkCaptureContext alloc] init];
        context.filters = [NSMutableDictionary dictionary];
        return (uintptr_t)CFBridgingRetain(context);
    }
}

void NmkReleaseVisionCaptureContext(uintptr_t handle) {
    @autoreleasepool {
        if (handle != 0) { id owner = CFBridgingRelease((void *)handle); (void)owner; }
    }
}

static NmkCaptureFilter *captureFilter(SCDisplay *display, NSArray<SCRunningApplication *> *excluded) {
    NmkCaptureFilter *entry = [[NmkCaptureFilter alloc] init];
    entry.filter = [[SCContentFilter alloc] initWithDisplay:display
        excludingApplications:excluded exceptingWindows:@[]];
    entry.bounds = CGDisplayBounds(display.displayID);
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display.displayID);
    if (mode != NULL) {
        entry.pixelWidth = CGDisplayModeGetPixelWidth(mode);
        entry.pixelHeight = CGDisplayModeGetPixelHeight(mode);
        CGDisplayModeRelease(mode);
    }
    return entry;
}

static bool captureFilterIsCurrent(NmkCaptureFilter *entry, CGDirectDisplayID displayID, CGRect bounds) {
    if (entry == nil || !CGRectEqualToRect(entry.bounds, bounds)) return false;
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(displayID);
    bool current = mode != NULL && entry.pixelWidth == CGDisplayModeGetPixelWidth(mode)
        && entry.pixelHeight == CGDisplayModeGetPixelHeight(mode);
    if (mode != NULL) CGDisplayModeRelease(mode);
    return current;
}

static CGImageRef captureRegion(
    CGRect region,
    uint64_t timeoutMS,
    uint64_t maximumPixels,
    uint64_t maximumDimension,
    uint64_t scanID,
    uintptr_t captureContext,
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
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW,
        (int64_t)MIN(MAX(timeoutMS, 1), 30000) * NSEC_PER_MSEC);
    *status = acquireScanCapture(scanID, deadline);
    if (*status != NMK_VISION_OK) {
        *message = *status == NMK_VISION_TIMEOUT ? @"Waiting for the previous screen capture timed out" : nil;
        return NULL;
    }

    dispatch_group_t group = dispatch_group_create();
    dispatch_semaphore_t wake = dispatch_semaphore_create(0);
    pthread_mutex_lock(&visionRequestLock);
    if (!scanIsCurrent(scanID)) {
        pthread_mutex_unlock(&visionRequestLock);
        releaseCapture();
        *status = NMK_VISION_CONTEXT_CHANGED;
        return NULL;
    }
    activeCaptureWake = wake;
    activeCaptureScan = scanID;
    pthread_mutex_unlock(&visionRequestLock);
    __block CGImageRef captured = NULL;
    __block NSError *captureError = nil;
    dispatch_group_enter(group);
    NmkCaptureContext *context = (__bridge NmkCaptureContext *)(void *)captureContext;
    void (^capture)(SCContentFilter *, NSError *) = ^(SCContentFilter *filter, NSError *error) {
        if (!scanIsCurrent(scanID) || error != nil || filter == nil) {
            captureError = error;
            releaseCapture();
            dispatch_group_leave(group);
            dispatch_semaphore_signal(wake);
            return;
        }
        SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
        // Quartz reports logical mode pixels on scaled Retina displays.
        // ScreenCaptureKit supplies the capture's actual point-to-pixel scale.
        CGFloat pixelScale = filter.pointPixelScale;
        configuration.sourceRect = CGRectMake(
            clipped.origin.x - displayBounds.origin.x,
            clipped.origin.y - displayBounds.origin.y,
            clipped.size.width,
            clipped.size.height);
        double captureWidth = MAX(1.0, clipped.size.width * pixelScale);
        double captureHeight = MAX(1.0, clipped.size.height * pixelScale);
        double scale = 1.0;
        if (maximumDimension > 0) {
            scale = MIN(scale, (double)maximumDimension / captureWidth);
            scale = MIN(scale, (double)maximumDimension / captureHeight);
        }
        if (maximumPixels > 0 && captureWidth * captureHeight > (double)maximumPixels) {
            scale = MIN(scale, sqrt((double)maximumPixels / (captureWidth * captureHeight)));
        }
        scale = MIN(1.0, scale);
        configuration.width = MAX(1, (size_t)llround(captureWidth * scale));
        configuration.height = MAX(1, (size_t)llround(captureHeight * scale));
        configuration.showsCursor = NO;
        configuration.colorSpaceName = kCGColorSpaceSRGB;
        [SCScreenshotManager captureImageWithFilter:filter configuration:configuration completionHandler:^(CGImageRef image, NSError *error) {
            captureError = error;
            if (error != nil && context != nil) {
                @synchronized(context) { [context.filters removeObjectForKey:@(displayID)]; }
            }
            if (image != NULL) captured = CGImageRetain(image);
            releaseCapture();
            dispatch_group_leave(group);
            dispatch_semaphore_signal(wake);
        }];
    };
    NmkCaptureFilter *cached = nil;
    if (context != nil) { @synchronized(context) { cached = context.filters[@(displayID)]; } }
    if (captureFilterIsCurrent(cached, displayID, displayBounds)) {
        capture(cached.filter, nil);
    } else {
        [SCShareableContent getShareableContentExcludingDesktopWindows:YES
            onScreenWindowsOnly:YES completionHandler:^(SCShareableContent *content, NSError *error) {
            if (!scanIsCurrent(scanID) || error != nil || content.displays.count == 0) {
                capture(nil, error);
                return;
            }
            NSMutableArray<SCRunningApplication *> *excluded = [NSMutableArray array];
            for (SCRunningApplication *app in content.applications) {
                if (app.processID == getpid()) [excluded addObject:app];
            }
            NSMutableDictionary *filters = [NSMutableDictionary dictionary];
            for (SCDisplay *display in content.displays) {
                filters[@(display.displayID)] = captureFilter(display, excluded);
            }
            if (context != nil) { @synchronized(context) { context.filters = filters; } }
            NmkCaptureFilter *entry = filters[@(displayID)];
            capture(entry.filter, nil);
        }];
    }

    bool timedOut = dispatch_semaphore_wait(wake, deadline) != 0;
    pthread_mutex_lock(&visionRequestLock);
    if (activeCaptureWake == wake) activeCaptureWake = nil;
    pthread_mutex_unlock(&visionRequestLock);
    // Cancellation can signal before ScreenCaptureKit's callback. Only that
    // callback completes the group and makes its captured/error fields readable.
    bool completed = dispatch_group_wait(group, DISPATCH_TIME_NOW) == 0;
    int32_t waitStatus = !scanIsCurrent(scanID) || (!timedOut && !completed) ? NMK_VISION_CONTEXT_CHANGED
        : timedOut ? NMK_VISION_TIMEOUT : NMK_VISION_OK;
    if (waitStatus != NMK_VISION_OK) {
        // The ScreenCaptureKit operation cannot be cancelled. Release a late
        // image after its completion block leaves the group instead of leaking
        // the retained CGImage when the caller has already timed out.
        dispatch_group_notify(group, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
            if (captured != NULL) CGImageRelease(captured);
        });
        *status = waitStatus;
        *message = waitStatus == NMK_VISION_TIMEOUT ? @"Screen capture timed out" : nil;
        return NULL;
    }
    if (captured == NULL) {
        bool denied = [captureError.domain isEqualToString:SCStreamErrorDomain]
            && captureError.code == SCStreamErrorUserDeclined;
        *status = denied ? NMK_VISION_PERMISSION : NMK_VISION_FAILED;
        *message = denied
            ? @"Screen Recording access was denied. If KeySteer is already enabled in System Settings, quit and reopen KeySteer from Applications."
            : captureError.localizedDescription ?: @"ScreenCaptureKit did not return an image";
    }
    return captured;
}

void NmkFreeVisionResult(NmkVisionResult *result);

// A point sampler retains only a display filter, a tiny configuration and one
// sRGB pixel context. Capture callbacks own their state across bounded waits.
@interface NmkPointSampler : NSObject {
@public
    uint8_t pixel[4];
    CGContextRef context;
}
@property(nonatomic, strong) NmkCaptureFilter *filter;
@property(nonatomic) CGDirectDisplayID displayID;
@end

@implementation NmkPointSampler
- (instancetype)init {
    self = [super init];
    if (self) {
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        context = CGBitmapContextCreate(pixel, 1, 1, 8, 4, space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(space);
        if (context == NULL) return nil;
    }
    return self;
}
- (void)dealloc { if (context != NULL) CGContextRelease(context); }
@end

bool NmkSamplePoint(void **owner, double x, double y, bool reset, uint8_t *rgba) {
    @autoreleasepool {
        if (reset) {
            if (*owner != NULL) { id old = CFBridgingRelease(*owner); *owner = NULL; (void)old; }
            return false;
        }
        if (!isfinite(x) || !isfinite(y) || !CGPreflightScreenCaptureAccess()) return false;
        CGDirectDisplayID displayID = 0;
        uint32_t count = 0;
        if (CGGetDisplaysWithPoint(CGPointMake(x, y), 1, &displayID, &count) != kCGErrorSuccess || count == 0) return false;
        if (*owner == NULL) *owner = (void *)CFBridgingRetain([[NmkPointSampler alloc] init]);
        NmkPointSampler *state = (__bridge NmkPointSampler *)*owner;
        if (state == nil || !tryAcquireCapture()) return false;
        CGRect bounds = CGDisplayBounds(displayID);
        dispatch_group_t group = dispatch_group_create();
        dispatch_group_enter(group);
        __block CGImageRef captured = NULL;
        __block NmkPointCapture region = {0};
        void (^capture)(SCContentFilter *) = ^(SCContentFilter *filter) {
            if (filter == nil || !NmkPlanPointCapture(x, y, bounds.origin.x, bounds.origin.y,
                bounds.size.width, bounds.size.height, filter.pointPixelScale, &region)) {
                releaseCapture(); dispatch_group_leave(group); return;
            }
            SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
            configuration.sourceRect = CGRectMake(region.x, region.y,
                region.source_width, region.source_height);
            configuration.width = region.width;
            configuration.height = region.height;
            configuration.destinationRect = CGRectMake(0, 0, region.width, region.height);
            configuration.scalesToFit = NO;
            configuration.showsCursor = NO;
            [SCScreenshotManager captureImageWithFilter:filter configuration:configuration completionHandler:^(CGImageRef image, NSError *error) {
                if (error == nil && image != NULL) captured = CGImageRetain(image);
                if (error != nil || image == NULL) {
                    @synchronized(state) {
                        if (state.filter.filter == filter) state.filter = nil;
                    }
                }
                releaseCapture();
                dispatch_group_leave(group);
            }];
        };
        NmkCaptureFilter *cached = nil;
        @synchronized(state) { if (state.displayID == displayID) cached = state.filter; }
        if (captureFilterIsCurrent(cached, displayID, bounds)) capture(cached.filter);
        else [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
            SCDisplay *display = nil;
            for (SCDisplay *item in content.displays) if (item.displayID == displayID) { display = item; break; }
            if (error != nil || display == nil) { capture(nil); return; }
            NSMutableArray<SCRunningApplication *> *excluded = [NSMutableArray array];
            for (SCRunningApplication *app in content.applications) if (app.processID == getpid()) [excluded addObject:app];
            // Never cache a filter that could capture our own marker or panel.
            if (excluded.count == 0) { capture(nil); return; }
            NmkCaptureFilter *filter = captureFilter(display, excluded);
            @synchronized(state) { state.displayID = displayID; state.filter = filter; }
            capture(filter.filter);
        }];
        if (dispatch_group_wait(group, dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC)) != 0) {
            dispatch_group_notify(group, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
                if (captured != NULL) CGImageRelease(captured);
            });
            return false;
        }
        if (captured == NULL) return false;
        bool sampled = NmkReadPointPixel(captured, state->context, region);
        CGImageRelease(captured);
        if (sampled) memcpy(rgba, state->pixel, 4);
        return sampled;
    }
}

NmkVisionResult *NmkDetectVisionElements(
    CGRect windowBounds,
    NmkVisionConfig config,
    uint64_t scanID,
    uintptr_t captureContext) {
    @autoreleasepool {
        config.timeout_ms = MIN(MAX(config.timeout_ms, 1), 30000);
        if (!scanIsCurrent(scanID)) {
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        // This capture follows the user's Hint/Text Select action. Let
        // ScreenCaptureKit enforce access and report its actual result: the
        // legacy CG preflight can disagree with capture after an app update.
        // Do not request/reset TCC here or cache a failed preflight indefinitely.
        int32_t captureStatus = NMK_VISION_OK;
        NSString *captureMessage = nil;
        CGRect capturedBounds = CGRectZero;
        CGImageRef image = captureRegion(
            windowBounds,
            config.timeout_ms,
            config.maximum_capture_pixels,
            config.maximum_capture_dimension,
            scanID,
            captureContext,
            &captureStatus,
            &captureMessage,
            &capturedBounds);
        if (image == NULL) return resultWithStatus(captureStatus, captureMessage);
        if (!scanIsCurrent(scanID)) {
            CGImageRelease(image);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }

        NmkVisionResult *result = resultWithStatus(NMK_VISION_OK, nil);
        if (result == NULL) { CGImageRelease(image); return NULL; }
        result->captured_bounds = capturedBounds;
        result->image = image;
        if (!config.detect_text && !config.detect_rectangles && !config.detect_contours) {
            CGImageRelease(result->image);
            result->image = NULL;
        }
        if (!scanIsCurrent(scanID)) {
            NmkFreeVisionResult(result);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        return result;
    }
}

// ScreenCaptureKit's common BGRA path exposes immutable bytes to the shared
// bounded luma converter. The caller owns this CFData until release; no bitmap
// redraw or decoded full-frame cache is needed.
static CFDataRef copyVisionBgraPixels(CGImageRef image) {
    if (image == NULL || CGImageGetBitsPerPixel(image) != 32 || CGImageGetBitsPerComponent(image) != 8) return NULL;
    CGImageAlphaInfo alpha = CGImageGetAlphaInfo(image);
    if ((CGImageGetBitmapInfo(image) & kCGBitmapByteOrderMask) != kCGBitmapByteOrder32Little
        || (alpha != kCGImageAlphaPremultipliedFirst && alpha != kCGImageAlphaNoneSkipFirst)) return NULL;
    return CGDataProviderCopyData(CGImageGetDataProvider(image));
}

// The contour source borrows packed pixels through the existing result owner.
// Uncommon formats retain the color-managed luma fallback. Neither path owns a CGImage.
NmkVisionResult *NmkPrepareVisionContourImage(uintptr_t imageHandle, uint64_t scanID) {
    @autoreleasepool {
        CGImageRef image = (CGImageRef)imageHandle;
        if (image == NULL || !scanIsCurrent(scanID)) return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        NmkVisionResult *result = resultWithStatus(NMK_VISION_OK, nil);
        if (result == NULL) return NULL;
        result->pixel_owner = copyVisionBgraPixels(image);
        if (result->pixel_owner != NULL) {
            result->gray = (uint8_t *)CFDataGetBytePtr(result->pixel_owner);
            result->gray_width = CGImageGetWidth(image);
            result->gray_height = CGImageGetHeight(image);
            result->pixel_length = (uint64_t)CFDataGetLength(result->pixel_owner);
            result->pixel_stride = CGImageGetBytesPerRow(image);
            if (!scanIsCurrent(scanID)) {
                NmkFreeVisionResult(result);
                return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
            }
            return result;
        }
        size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
        double scale = MIN(1.0, MIN(2560.0 / MAX(width, height),
            sqrt(2073600.0 / ((double)width * height))));
        width = MAX(1, (size_t)floor(width * scale));
        height = MAX(1, (size_t)floor(height * scale));
        result->gray = calloc(width, height);
        CGColorSpaceRef space = CGColorSpaceCreateDeviceGray();
        CGContextRef context = result->gray != NULL && space != NULL
            ? CGBitmapContextCreate(result->gray, width, height, 8, width, space, (CGBitmapInfo)kCGImageAlphaNone)
            : NULL;
        if (space != NULL) CGColorSpaceRelease(space);
        if (context == NULL) {
            NmkFreeVisionResult(result);
            return resultWithStatus(NMK_VISION_FAILED, @"Cannot prepare contour luma frame");
        }
        // Bitmap row zero is the top row of the untransformed image.
        CGContextSetInterpolationQuality(context, kCGInterpolationLow);
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
        CGContextRelease(context);
        result->gray_width = width;
        result->gray_height = height;
        result->pixel_length = width * height;
        if (!scanIsCurrent(scanID)) {
            NmkFreeVisionResult(result);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        return result;
    }
}

// Each provider group owns a separate retain while its Vision calls are in
// flight. The capture result can be freed immediately after dispatch.
uintptr_t NmkRetainVisionImage(uintptr_t imageHandle) {
    CGImageRef image = (CGImageRef)imageHandle;
    return image != NULL ? (uintptr_t)CGImageRetain(image) : 0;
}

void NmkReleaseVisionImage(uintptr_t imageHandle) {
    if (imageHandle != 0) CGImageRelease((CGImageRef)imageHandle);
}

uint64_t NmkVisionImageWidth(uintptr_t imageHandle) {
    return imageHandle != 0 ? CGImageGetWidth((CGImageRef)imageHandle) : 0;
}

uint64_t NmkVisionImageHeight(uintptr_t imageHandle) {
    return imageHandle != 0 ? CGImageGetHeight((CGImageRef)imageHandle) : 0;
}

uintptr_t NmkCropVisionImage(uintptr_t imageHandle, uint64_t x, uint64_t y, uint64_t width, uint64_t height) {
    if (imageHandle == 0 || width == 0 || height == 0) return 0;
    CGImageRef image = (CGImageRef)imageHandle;
    size_t imageWidth = CGImageGetWidth(image), imageHeight = CGImageGetHeight(image);
    if (x >= imageWidth || y >= imageHeight || width > imageWidth - x || height > imageHeight - y) return 0;
    // Core Graphics retains the source backing store. Only in-flight tiles
    // own subimages, and cropping never resamples the screenshot's pixels.
    return (uintptr_t)CGImageCreateWithImageInRect(image, CGRectMake(x, y, width, height));
}

static VNRecognizeTextRequest *makeUiTextRequest(bool fastPreview) {
    VNRecognizeTextRequest *textRequest = [[VNRecognizeTextRequest alloc] init];
    textRequest.recognitionLevel = fastPreview
        ? VNRequestTextRecognitionLevelFast : VNRequestTextRecognitionLevelAccurate;
    // Revision 3 can spend over a minute compiling ANE models for a
    // freshly built app, even with an explicit CPU compute device.
    // Revision 2 supports all three configured languages and avoids
    // that startup path; keep Accurate recognition and full pixels.
    if ([VNRecognizeTextRequest.supportedRevisions containsIndex:VNRecognizeTextRequestRevision2]) {
        textRequest.revision = VNRecognizeTextRequestRevision2;
    }
    // Vision defaults to 1/32 of the image height. Ordinary webpage
    // text is much smaller on a full-window/Retina capture; do not
    // discard it before recognition. Capture size and scan deadlines
    // still bound the work, independently of AX and contours.
    textRequest.minimumTextHeight = 0.0f;
    // Preview text is never published: Fast cannot recognize Chinese. Its
    // geometry is provisional until the normal multilingual request returns.
    textRequest.recognitionLanguages = fastPreview
        ? @[@"en-US"] : @[@"zh-Hans", @"zh-Hant", @"en-US"];
    textRequest.automaticallyDetectsLanguage = !fastPreview;
    textRequest.usesLanguageCorrection = NO;
    return textRequest;
}

static NmkVisionResult *recognizeVisionImage(
    uintptr_t imageHandle, CGRect capturedBounds, NmkVisionConfig config, uint64_t scanID, bool fastPreview) {
    @autoreleasepool {
        CGImageRef image = (CGImageRef)imageHandle;
        if (image == NULL || !scanIsCurrent(scanID)) {
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
            textRequest = makeUiTextRequest(fastPreview);
            [requests addObject:textRequest];
        }

        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
        NSError *error = nil;
        if (!registerVisionRequests(rectangleRequest, textRequest, scanID)) {
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        BOOL performed = [handler performRequests:requests error:&error];
        unregisterVisionRequests(rectangleRequest, textRequest);
        if (!scanIsCurrent(scanID)) {
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        if (!performed || error != nil) {
            return resultWithStatus(NMK_VISION_FAILED, error.localizedDescription ?: @"Vision request failed");
        }

        NSArray<VNRectangleObservation *> *rectangleResults = rectangleRequest.results ?: @[];
        NSArray<VNRecognizedTextObservation *> *textResults = textRequest.results ?: @[];
        if (rectangleResults.count > SIZE_MAX - textResults.count) {
            return resultWithStatus(NMK_VISION_FAILED, @"Vision result size overflow");
        }
        NSUInteger capacity = rectangleResults.count + textResults.count;
        if (capacity > SIZE_MAX / sizeof(NmkVisionRegion)) {
            return resultWithStatus(NMK_VISION_FAILED, @"Vision result buffer size overflow");
        }
        NmkVisionResult *result = resultWithStatus(NMK_VISION_OK, nil);
        if (result == NULL) return NULL;
        result->captured_bounds = capturedBounds;
        result->regions = calloc(capacity, sizeof(NmkVisionRegion));
        if (capacity > 0 && result->regions == NULL) {
            free(result);
            return NULL;
        }

        for (VNRecognizedTextObservation *observation in textResults) {
            if ((result->count & 31) == 0 && !scanIsCurrent(scanID)) {
                NmkFreeVisionResult(result);
                return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
            }
            if (result->count >= capacity) break;
            if (observation.confidence < config.minimum_confidence) continue;
            CGRect box = observation.boundingBox;
            NmkVisionRegion *out = &result->regions[result->count++];
            out->x = box.origin.x;
            out->y = box.origin.y;
            out->width = box.size.width;
            out->height = box.size.height;
            out->confidence = observation.confidence;
            out->is_text = true;
            if (!fastPreview) {
                VNRecognizedText *candidate = [observation topCandidates:1].firstObject;
                NSString *label = candidate.string ?: @"";
                const char *utf8 = label.UTF8String;
                if (utf8 != NULL) {
                    out->label_len = (uint64_t)strlen(utf8);
                    out->label = strdup(utf8);
                }
            }
        }
        for (VNRectangleObservation *observation in rectangleResults) {
            if ((result->count & 31) == 0 && !scanIsCurrent(scanID)) {
                NmkFreeVisionResult(result);
                return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
            }
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
            out->label_len = 0;
        }
        if (!scanIsCurrent(scanID)) {
            NmkFreeVisionResult(result);
            return resultWithStatus(NMK_VISION_CONTEXT_CHANGED, nil);
        }
        return result;
    }
}

NmkVisionResult *NmkRecognizeVisionImage(
    uintptr_t imageHandle, CGRect capturedBounds, NmkVisionConfig config, uint64_t scanID) {
    return recognizeVisionImage(imageHandle, capturedBounds, config, scanID, false);
}

void NmkFreeVisionResult(NmkVisionResult *result) {
    if (result == NULL) return;
    for (uint64_t index = 0; index < result->count; index++) free(result->regions[index].label);
    if (result->image != NULL) CGImageRelease(result->image);
    if (result->pixel_owner != NULL) CFRelease(result->pixel_owner);
    else free(result->gray);
    free(result->regions);
    free(result->message);
    free(result);
}

#include "vision_ocr_process.h"
