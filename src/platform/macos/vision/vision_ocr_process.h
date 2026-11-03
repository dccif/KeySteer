// Private helper mode of the same signed executable. No application/runtime
// initialization, screen capture, temporary images or extra packaged binary.
#include <errno.h>
#include <fcntl.h>
#include <sys/event.h>
#include <signal.h>
#include <stdio.h>

static bool validOcrImageSize(uint64_t width, uint64_t height) {
    // Capture rounds each scaled dimension independently. Accept that bounded
    // edge rounding instead of rejecting otherwise valid Retina screenshots.
    return width > 0 && height > 0 && width <= 4096 && height <= 4096
        && width * height <= 8388608 + 2 * 4096 + 1;
}

static bool writeOcrBytes(int fd, const void *bytes, size_t size) {
    const uint8_t *cursor = bytes;
    while (size > 0) {
        ssize_t written = write(fd, cursor, size);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) return false;
        cursor += written;
        size -= (size_t)written;
    }
    return true;
}

// ScreenCaptureKit supplies packed BGRA. Read its provider directly instead of
// drawing a second full image into a bitmap context (which can retain large
// decoded-image caches). A bounded scratch buffer normalizes the IPC to RGBA.
static int writePackedOcrImage(CGImageRef image, int fd, uint64_t scanID) {
    CGBitmapInfo info = CGImageGetBitmapInfo(image);
    CGImageAlphaInfo alpha = CGImageGetAlphaInfo(image);
    bool bgra = (info & kCGBitmapByteOrderMask) == kCGBitmapByteOrder32Little
        && (alpha == kCGImageAlphaPremultipliedFirst || alpha == kCGImageAlphaNoneSkipFirst);
    bool rgba = (info & kCGBitmapByteOrderMask) == kCGBitmapByteOrder32Big
        && (alpha == kCGImageAlphaPremultipliedLast || alpha == kCGImageAlphaNoneSkipLast);
    CGColorSpaceRef colorSpace = CGImageGetColorSpace(image);
    if ((!bgra && !rgba) || CGImageGetBitsPerComponent(image) != 8
        || CGImageGetBitsPerPixel(image) != 32
        || colorSpace == NULL || CGColorSpaceGetModel(colorSpace) != kCGColorSpaceModelRGB) return -1;
    uint64_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
    size_t stride = CGImageGetBytesPerRow(image);
    if (stride < width * 4 || height > SIZE_MAX / stride) return -1;
    CFDataRef data = CGDataProviderCopyData(CGImageGetDataProvider(image));
    if (data == NULL) return -1;
    if ((uint64_t)CFDataGetLength(data) < height * stride) { CFRelease(data); return -1; }
    const uint8_t *pixels = CFDataGetBytePtr(data);
    bool opaque = alpha == kCGImageAlphaNoneSkipFirst || alpha == kCGImageAlphaNoneSkipLast;
    bool ok = fcntl(fd, F_SETNOSIGPIPE, 1) == 0 && scanIsCurrent(scanID)
        && writeOcrBytes(fd, &width, sizeof(width)) && writeOcrBytes(fd, &height, sizeof(height));
    uint8_t scratch[65536];
    // The validated width is at most 4096 pixels, so whole rows fit. Pack
    // several rows per write without transmitting source padding or allocating
    // another frame; a 4K capture uses about a quarter as many pipe writes.
    size_t rowBytes = (size_t)width * 4;
    for (uint64_t row = 0; row < height && ok;) {
        size_t rows = MIN(height - row, sizeof(scratch) / rowBytes);
        for (size_t y = 0; y < rows; y++) {
            const uint8_t *source = pixels + (row + y) * stride;
            uint8_t *destination = scratch + y * rowBytes;
            for (size_t pixel = 0; pixel < width; pixel++) {
                destination[pixel * 4] = source[pixel * 4 + (bgra ? 2 : 0)];
                destination[pixel * 4 + 1] = source[pixel * 4 + 1];
                destination[pixel * 4 + 2] = source[pixel * 4 + (bgra ? 0 : 2)];
                destination[pixel * 4 + 3] = opaque ? 255 : source[pixel * 4 + 3];
            }
        }
        ok = scanIsCurrent(scanID) && writeOcrBytes(fd, scratch, rows * rowBytes);
        row += rows;
    }
    CFRelease(data);
    return ok ? 1 : 0;
}

bool NmkWriteVisionOcrImage(uintptr_t imageHandle, int fd, uint64_t scanID) {
    CGImageRef image = (CGImageRef)imageHandle;
    if (image == NULL || !scanIsCurrent(scanID)) return false;
    uint64_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
    if (!validOcrImageSize(width, height)) return false;
    int packed = writePackedOcrImage(image, fd, scanID);
    if (packed >= 0) return packed != 0;
    size_t size = (size_t)(width * height * 4);
    uint8_t *pixels = malloc(size);
    if (pixels == NULL) return false;
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGContextRef context = space != NULL
        ? CGBitmapContextCreate(pixels, width, height, 8, width * 4, space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big) : NULL;
    if (space != NULL) CGColorSpaceRelease(space);
    if (context == NULL) { free(pixels); return false; }
    CGContextSetBlendMode(context, kCGBlendModeCopy);
    CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
    CGContextRelease(context);
    // Cancellation may close the reader during transfer. Suppress SIGPIPE
    // on this descriptor without changing the application's signal handlers.
    bool ok = fcntl(fd, F_SETNOSIGPIPE, 1) == 0 && scanIsCurrent(scanID)
        && writeOcrBytes(fd, &width, sizeof(width))
        && writeOcrBytes(fd, &height, sizeof(height))
        && writeOcrBytes(fd, pixels, size);
    free(pixels);
    return ok;
}

static void releaseOcrPixels(void *info, const void *bytes, size_t size) {
    (void)info;
    (void)size;
    free((void *)bytes);
}

static void *watchOcrParent(void *unused) {
    (void)unused;
    // Observe pipe EOF without consuming the next frame's bytes. EV_CLEAR
    // avoids spinning on unread input while Vision is busy or stuck.
    int queue = kqueue();
    if (queue < 0) _exit(1);
    struct kevent input, event;
    EV_SET(&input, STDIN_FILENO, EVFILT_READ, EV_ADD | EV_CLEAR, 0, 0, NULL);
    if (kevent(queue, &input, 1, NULL, 0, NULL) < 0) _exit(1);
    for (;;) {
        int count = kevent(queue, NULL, 0, &event, 1, NULL);
        if (count < 0 && errno == EINTR) continue;
        if (count < 0 || (count > 0 && (event.flags & (EV_EOF | EV_ERROR)))) break;
    }
    _exit(1);
}

static bool writeOcrMessage(NSDictionary *message) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:message options:0 error:NULL];
    return data != nil && writeOcrBytes(STDOUT_FILENO, data.bytes, data.length)
        && writeOcrBytes(STDOUT_FILENO, "\n", 1);
}

static int32_t recognizeOcrFrame(void) {
    @autoreleasepool {
        char metadata[65536];
        if (fgets(metadata, sizeof(metadata), stdin) == NULL || strchr(metadata, '\n') == NULL) return 1;
        NSData *json = [NSData dataWithBytes:metadata length:strlen(metadata)];
        id decoded = [NSJSONSerialization JSONObjectWithData:json options:0 error:NULL];
        if (![decoded isKindOfClass:NSDictionary.class]) return 1;
        NSDictionary *options = decoded;
        NSArray *tiles = options[@"tiles"];
        NSNumber *parallelism = options[@"parallelism"], *confidence = options[@"minimum_confidence"];
        id previewOption = options[@"fast_preview"];
        if (previewOption != nil && ![previewOption isKindOfClass:NSNumber.class]) return 1;
        bool fastPreview = [previewOption boolValue];
        if (![tiles isKindOfClass:NSArray.class] || tiles.count == 0
            || ![parallelism isKindOfClass:NSNumber.class] || ![confidence isKindOfClass:NSNumber.class]) return 1;
        size_t lanes = MIN(MIN(parallelism.unsignedIntegerValue, NMK_MAX_OCR_IN_FLIGHT), tiles.count);
        if (lanes == 0) return 1;
        uint64_t width = 0, height = 0;
        if (fread(&width, sizeof(width), 1, stdin) != 1 || fread(&height, sizeof(height), 1, stdin) != 1
            || !validOcrImageSize(width, height)) return 1;
        size_t size = (size_t)(width * height * 4);
        uint8_t *pixels = malloc(size);
        if (pixels == NULL) return 1;
        if (fread(pixels, 1, size, stdin) != size) { free(pixels); return 1; }
        CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, pixels, size, releaseOcrPixels);
        if (provider == NULL) { free(pixels); return 1; }
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGImageRef image = space != NULL
            ? CGImageCreate(width, height, 8, 32, width * 4, space,
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
                provider, NULL, false, kCGRenderingIntentDefault) : NULL;
        if (space != NULL) CGColorSpaceRelease(space);
        CGDataProviderRelease(provider);
        if (image == NULL) return 1;
        NmkSetLatestVisionScan(1);
        NSLock *output = [[NSLock alloc] init];
        // Finish the fast pass across the whole frame before any lane enters
        // slow multilingual OCR. The image backing is shared by both passes.
        for (unsigned pass = fastPreview ? 0 : 1; pass < 2; pass++) {
            _Atomic size_t next = 0;
            _Atomic size_t *cursor = &next;
            dispatch_apply(lanes, dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^(size_t lane) {
                (void)lane;
                size_t index;
                while ((index = atomic_fetch_add_explicit(cursor, 1, memory_order_relaxed)) < tiles.count) {
                    @autoreleasepool {
                        id tile = tiles[index];
                        if (![tile isKindOfClass:NSArray.class] || [tile count] != 4) _exit(1);
                        for (id value in tile) if (![value isKindOfClass:NSNumber.class]) _exit(1);
                        uintptr_t crop = NmkCropVisionImage((uintptr_t)image,
                            [tile[0] unsignedLongLongValue], [tile[1] unsignedLongLongValue],
                            [tile[2] unsignedLongLongValue], [tile[3] unsignedLongLongValue]);
                        if (crop == 0) _exit(1);
#ifdef NMK_VISION_TESTING
                        if (getenv("KEYSTEER_TEST_OCR_HANG") != NULL) {
                            [output lock];
                            bool sent = writeOcrMessage(@{@"kind": @"blocked", @"index": @(index)});
                            [output unlock];
                            if (!sent) _exit(1);
                            // Fault injection deliberately never returns, even
                            // after cooperative VNRequest cancellation.
                            for (;;) pause();
                        }
#endif
                        NmkVisionConfig config = {.detect_text = true, .minimum_confidence = confidence.doubleValue};
                        NmkVisionResult *result = recognizeVisionImage(crop, CGRectZero, config, 1, pass == 0);
                        if (result == NULL) _exit(1);
                        NSMutableArray *regions = [NSMutableArray array];
                        for (uint64_t region = 0; region < result->count; region++) {
                            NmkVisionRegion *value = &result->regions[region];
                            NSString *label = pass == 0 ? @"" :
                                ([[NSString alloc] initWithBytes:value->label
                                    length:(NSUInteger)value->label_len encoding:NSUTF8StringEncoding] ?: @"");
                            [regions addObject:@{@"x": @(value->x), @"y": @(value->y),
                                @"width": @(value->width), @"height": @(value->height),
                                @"confidence": @(value->confidence), @"label": label}];
                        }
                        NSString *message = result->message != NULL
                            ? [[NSString alloc] initWithBytes:result->message length:(NSUInteger)result->message_len
                                encoding:NSUTF8StringEncoding] ?: @"" : @"";
                        NSDictionary *record = @{@"kind": pass == 0 ? @"preview" : @"tile", @"index": @(index),
                            @"status": @(result->status), @"regions": regions, @"message": message};
                        [output lock];
                        bool sent = writeOcrMessage(record);
                        [output unlock];
                        NmkFreeVisionResult(result);
                        if (!sent) _exit(1);
                        NmkReleaseVisionImage(crop);
                    }
                }
            });
        }
        CGImageRelease(image);
        return writeOcrMessage(@{@"kind": @"done"}) ? 0 : 1;
    }
}

int32_t NmkRunVisionOcrProcess(void) {
    signal(SIGPIPE, SIG_IGN);
    pthread_t parentWatcher;
    if (pthread_create(&parentWatcher, NULL, watchOcrParent, NULL) != 0) return 1;
    pthread_detach(parentWatcher);
    // Each frame drains its pixels, requests and results before accepting the
    // next header. Only Vision's process-local model state survives reuse.
    while (recognizeOcrFrame() == 0) {}
    return 1;
}
