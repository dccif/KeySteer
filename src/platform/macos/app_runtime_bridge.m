#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>

#include <stdbool.h>
#include <float.h>
#include <math.h>

typedef void (*KskAppRuntimeCallback)(void);

@interface KskAppRuntimeDriver : NSObject <NSApplicationDelegate> {
@private
    CFRunLoopObserverRef _observer;
    CFRunLoopTimerRef _timer;
    KskAppRuntimeCallback _didFinishLaunching;
    KskAppRuntimeCallback _drive;
    BOOL _installed;
}

- (instancetype)initWithDidFinishLaunching:(KskAppRuntimeCallback)didFinishLaunching
                                      drive:(KskAppRuntimeCallback)drive
                                  errorCode:(int *)errorCode;
- (void)install;
- (void)invalidate;
- (void)driveEngine;
- (void)scheduleAfter:(double)seconds;

@end

static KskAppRuntimeDriver *KskRuntimeDriver;

static void KskObserveMainRunLoop(CFRunLoopObserverRef observer,
                                  CFRunLoopActivity activity,
                                  void *info) {
    (void)observer;
    (void)activity;
    KskAppRuntimeDriver *driver = (__bridge KskAppRuntimeDriver *)info;
    [driver driveEngine];
}

static void KskDeadlineTimerFired(CFRunLoopTimerRef timer, void *info) {
    (void)timer;
    (void)info;
    // The before-waiting observer drains the engine after this source fires.
}

@implementation KskAppRuntimeDriver

- (instancetype)initWithDidFinishLaunching:(KskAppRuntimeCallback)didFinishLaunching
                                      drive:(KskAppRuntimeCallback)drive
                                  errorCode:(int *)errorCode {
    self = [super init];
    if (self == nil) return nil;

    _didFinishLaunching = didFinishLaunching;
    _drive = drive;

    CFRunLoopObserverContext observerContext = {
        0,
        (__bridge void *)self,
        NULL,
        NULL,
        NULL,
    };
    _observer = CFRunLoopObserverCreate(
        kCFAllocatorDefault,
        kCFRunLoopAfterWaiting | kCFRunLoopBeforeWaiting,
        true,
        0,
        KskObserveMainRunLoop,
        &observerContext);
    if (_observer == NULL) {
        if (errorCode != NULL) *errorCode = 3;
        return nil;
    }

    CFRunLoopTimerContext timerContext = {0, NULL, NULL, NULL, NULL};
    CFAbsoluteTime dormantFire = CFAbsoluteTimeGetCurrent() + 24.0 * 60.0 * 60.0;
    _timer = CFRunLoopTimerCreate(
        kCFAllocatorDefault,
        dormantFire,
        24.0 * 60.0 * 60.0,
        0,
        0,
        KskDeadlineTimerFired,
        &timerContext);
    if (_timer == NULL) {
        if (errorCode != NULL) *errorCode = 4;
        CFRelease(_observer);
        _observer = NULL;
        return nil;
    }

    return self;
}

- (void)install {
    if (_installed) return;
    _installed = YES;
    CFRunLoopRef runLoop = CFRunLoopGetMain();
    CFRunLoopAddObserver(runLoop, _observer, kCFRunLoopCommonModes);
    CFRunLoopAddTimer(runLoop, _timer, kCFRunLoopCommonModes);
    NSApplication.sharedApplication.delegate = self;
}

- (void)invalidate {
    if (!_installed) return;
    _installed = NO;
    NSApplication *application = NSApplication.sharedApplication;
    if (application.delegate == self) application.delegate = nil;

    CFRunLoopRef runLoop = CFRunLoopGetMain();
    CFRunLoopRemoveObserver(runLoop, _observer, kCFRunLoopCommonModes);
    CFRunLoopRemoveTimer(runLoop, _timer, kCFRunLoopCommonModes);
    CFRunLoopObserverInvalidate(_observer);
    CFRunLoopTimerInvalidate(_timer);
}

- (void)dealloc {
    [self invalidate];
    if (_observer != NULL) CFRelease(_observer);
    if (_timer != NULL) CFRelease(_timer);
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    if (_didFinishLaunching != NULL) _didFinishLaunching();
}

- (void)driveEngine {
    if (_drive != NULL) _drive();
}

- (void)scheduleAfter:(double)seconds {
    CFRunLoopTimerSetNextFireDate(
        _timer,
        CFAbsoluteTimeGetCurrent() + seconds);
}

@end

int KskInstallAppRuntimeDriver(KskAppRuntimeCallback didFinishLaunching,
                               KskAppRuntimeCallback drive) {
    @autoreleasepool {
        if (!NSThread.isMainThread) return 1;
        if (KskRuntimeDriver != nil) return 2;

        int errorCode = 0;
        KskAppRuntimeDriver *driver =
            [[KskAppRuntimeDriver alloc] initWithDidFinishLaunching:didFinishLaunching
                                                              drive:drive
                                                          errorCode:&errorCode];
        if (driver == nil) return errorCode != 0 ? errorCode : 3;
        KskRuntimeDriver = driver;
        [driver install];
        return 0;
    }
}

void KskRunApplication(void) {
    // NSApplication owns an autorelease pool per dispatched event. Do not keep
    // one outer pool alive for the entire process lifetime.
    if (KskRuntimeDriver != nil) [NSApplication.sharedApplication run];
}

void KskScheduleAppRuntime(double afterSeconds) {
    if (KskRuntimeDriver == nil || !isfinite(afterSeconds)) return;
    double boundedSeconds = afterSeconds > 0.0 ? afterSeconds : DBL_EPSILON;
    [KskRuntimeDriver scheduleAfter:boundedSeconds];
}

void KskStopApplication(void) {
    if (KskRuntimeDriver == nil) return;
    [NSApplication.sharedApplication stop:nil];
    CFRunLoopWakeUp(CFRunLoopGetMain());
}

void KskDestroyAppRuntimeDriver(void) {
    @autoreleasepool {
        KskAppRuntimeDriver *driver = KskRuntimeDriver;
        if (driver == nil) return;
        [driver invalidate];
        KskRuntimeDriver = nil;
    }
}
