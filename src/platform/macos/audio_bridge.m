// Worker-owned Core Audio controls. Per-app audio stays in a bounded in-memory
// tap -> float stereo ring -> AVAudioEngine path; no files or network access.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <AudioToolbox/AudioToolbox.h>
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef void (*KSAudioLog)(const char *message);

static AudioObjectPropertyAddress Address(AudioObjectPropertySelector selector,
                                          AudioObjectPropertyScope scope, UInt32 element) {
    return (AudioObjectPropertyAddress){ selector, scope, element };
}
static OSStatus Read(AudioObjectID object, AudioObjectPropertySelector selector,
                     AudioObjectPropertyScope scope, UInt32 element, UInt32 *size, void *value) {
    AudioObjectPropertyAddress address = Address(selector, scope, element);
    return AudioObjectGetPropertyData(object, &address, 0, NULL, size, value);
}
static OSStatus Write(AudioObjectID object, AudioObjectPropertySelector selector,
                      AudioObjectPropertyScope scope, UInt32 element, UInt32 size, const void *value) {
    AudioObjectPropertyAddress address = Address(selector, scope, element);
    return AudioObjectSetPropertyData(object, &address, 0, NULL, size, value);
}
static NSString *Failure(NSString *operation, OSStatus status) {
    return [NSString stringWithFormat:@"%@: Core Audio error %d", operation, (int)status];
}
static AudioDeviceID DefaultOutput(void) {
    AudioDeviceID device = kAudioObjectUnknown; UInt32 size = sizeof(device);
    Read(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice,
         kAudioObjectPropertyScopeGlobal, 0, &size, &device);
    return device;
}
static NSString *StringProperty(AudioObjectID object, AudioObjectPropertySelector selector) {
    CFStringRef value = NULL; UInt32 size = sizeof(value);
    if (Read(object, selector, kAudioObjectPropertyScopeGlobal, 0, &size, &value) != noErr) return nil;
    return CFBridgingRelease(value);
}
static UInt32 OutputChannels(AudioDeviceID device) {
    AudioObjectPropertyAddress address = Address(kAudioDevicePropertyStreamConfiguration,
                                                kAudioDevicePropertyScopeOutput, 0);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size) != noErr || size < sizeof(AudioBufferList)) return 0;
    AudioBufferList *list = calloc(1, size); if (!list) return 0;
    UInt32 channels = 0;
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, list) == noErr) {
        size_t required = offsetof(AudioBufferList, mBuffers) + (size_t)list->mNumberBuffers * sizeof(AudioBuffer);
        if (required <= size) for (UInt32 i = 0; i < list->mNumberBuffers; ++i) channels += list->mBuffers[i].mNumberChannels;
    }
    free(list); return channels;
}
static NSArray<NSDictionary *> *Outputs(void) {
    AudioObjectPropertyAddress address = Address(kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal, 0);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, NULL, &size) != noErr) return @[];
    AudioDeviceID *ids = calloc(1, size); if (!ids) return @[];
    NSMutableArray *outputs = [NSMutableArray array];
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, ids) == noErr) {
        for (UInt32 i = 0; i < size / sizeof(*ids); ++i) {
            UInt32 alive = 0, bytes = sizeof(alive);
            if (Read(ids[i], kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, 0, &bytes, &alive) != noErr || !alive || !OutputChannels(ids[i])) continue;
            NSString *uid = StringProperty(ids[i], kAudioDevicePropertyDeviceUID);
            if (!uid || [uid hasPrefix:@"com.keysteer.tap."]) continue;
            [outputs addObject:@{ @"id": @(ids[i]), @"uid": uid,
                @"name": StringProperty(ids[i], kAudioObjectPropertyName) ?: uid }];
        }
    }
    free(ids);
    return [outputs sortedArrayUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) {
        NSComparisonResult order = [a[@"name"] localizedCaseInsensitiveCompare:b[@"name"]];
        return order == NSOrderedSame ? [a[@"uid"] compare:b[@"uid"]] : order;
    }];
}
static NSDictionary *NextOutput(NSArray<NSDictionary *> *outputs, NSString *current, bool previous) {
    if (!outputs.count) return nil;
    NSUInteger index = [outputs indexOfObjectPassingTest:^BOOL(NSDictionary *value, NSUInteger i, BOOL *stop) {
        (void)i;
        (void)stop;
        return [value[@"uid"] isEqualToString:current];
    }];
    if (index == NSNotFound) index = previous ? outputs.count - 1 : 0;
    else index = (index + (previous ? outputs.count - 1 : 1)) % outputs.count;
    return outputs[index];
}
static NSString *SystemChange(uint32_t action, bool *ok) {
    AudioDeviceID device = DefaultOutput();
    if (!device) return @"No default audio output device";
    if (action >= 3) {
        NSDictionary *next = NextOutput(Outputs(), StringProperty(device, kAudioDevicePropertyDeviceUID), action == 3);
        if (!next) return @"No active audio output devices";
        AudioDeviceID target = [next[@"id"] unsignedIntValue];
        AudioDeviceID alerts = 0; UInt32 bytes = sizeof(alerts);
        OSStatus status = Read(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultSystemOutputDevice,
                               kAudioObjectPropertyScopeGlobal, 0, &bytes, &alerts);
        if (status != noErr) return Failure(@"Cannot read system alert output", status);
        status = Write(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice,
                                kAudioObjectPropertyScopeGlobal, 0, sizeof(target), &target);
        if (status != noErr) return Failure(@"Cannot switch system output", status);
        status = Write(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultSystemOutputDevice,
                       kAudioObjectPropertyScopeGlobal, 0, sizeof(target), &target);
        if (status != noErr) {
            bool restored = Write(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice,
                                  kAudioObjectPropertyScopeGlobal, 0, sizeof(device), &device) == noErr;
            restored &= Write(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultSystemOutputDevice,
                              kAudioObjectPropertyScopeGlobal, 0, sizeof(alerts), &alerts) == noErr;
            return [Failure(@"Cannot switch system alert output", status) stringByAppendingString:(restored ? @"; restored" : @"; rollback incomplete")];
        }
        *ok = true; return [@"System output: " stringByAppendingString:next[@"name"]];
    }
    // Some devices expose a master control, others expose individual channels.
    AudioObjectPropertySelector selector = action == 2 ? kAudioDevicePropertyMute : kAudioDevicePropertyVolumeScalar;
    NSMutableArray<NSNumber *> *elements = [NSMutableArray array];
    UInt32 count = OutputChannels(device);
    if (count > 128) return @"Audio device has too many output channels";
    for (UInt32 element = 0; element <= count; ++element) {
        AudioObjectPropertyAddress address = Address(selector, kAudioDevicePropertyScopeOutput, element);
        Boolean settable = false;
        if (AudioObjectHasProperty(device, &address) && AudioObjectIsPropertySettable(device, &address, &settable) == noErr && settable) {
            [elements addObject:@(element)]; if (element == 0) break;
        }
    }
    if (!elements.count) return @"This output device has no software volume control";
    UInt32 old[128], changed = 0; Float32 maximum = 0; bool allMuted = true;
    for (NSUInteger i = 0; i < elements.count; ++i) {
        UInt32 bytes = sizeof(old[i]);
        OSStatus status = Read(device, selector, kAudioDevicePropertyScopeOutput, elements[i].unsignedIntValue, &bytes, &old[i]);
        if (status != noErr) return Failure(@"Cannot read system audio", status);
        allMuted &= old[i] != 0;
    }
    for (NSUInteger i = 0; i < elements.count; ++i) {
        UInt32 value;
        if (action == 2) value = !allMuted;
        else {
            Float32 current; memcpy(&current, &old[i], sizeof(current));
            current = fminf(1, fmaxf(0, current + (action == 1 ? .01f : -.01f)));
            maximum = fmaxf(maximum, current); memcpy(&value, &current, sizeof(value));
        }
        OSStatus status = Write(device, selector, kAudioDevicePropertyScopeOutput, elements[i].unsignedIntValue, sizeof(value), &value);
        if (status != noErr) {
            bool restored = true;
            for (NSUInteger j = 0; j <= changed; ++j) restored &= Write(device, selector, kAudioDevicePropertyScopeOutput, elements[j].unsignedIntValue, sizeof(old[j]), &old[j]) == noErr;
            return [Failure(@"Cannot change system audio", status) stringByAppendingString:(restored ? @"; restored" : @"; rollback incomplete")];
        }
        ++changed;
    }
    *ok = true;
    return action == 2 ? (allMuted ? @"System unmuted" : @"System muted") : [NSString stringWithFormat:@"System volume %.0f%%", maximum * 100];
}

// Single producer (tap IOProc), single consumer (source render block). Neither
// real-time callback allocates, locks, logs, calls Objective-C, nor frees state.
#define KS_AUDIO_FRAMES 32768u
typedef struct {
    _Atomic(uint32_t) read, write;
    _Atomic(float) gain;
    float samples[KS_AUDIO_FRAMES * 2];
} KSAudioRing;
static OSStatus Capture(AudioDeviceID device, const AudioTimeStamp *now,
                        const AudioBufferList *input, const AudioTimeStamp *inputTime,
                        AudioBufferList *output, const AudioTimeStamp *outputTime, void *context) {
    (void)device;
    (void)now;
    (void)inputTime;
    (void)output;
    (void)outputTime;
    KSAudioRing *ring = context;
    if (!input || !input->mNumberBuffers) return noErr;
    bool interleaved = input->mNumberBuffers == 1 && input->mBuffers[0].mNumberChannels == 2;
    bool planar = input->mNumberBuffers == 2 && input->mBuffers[0].mNumberChannels == 1 && input->mBuffers[1].mNumberChannels == 1;
    if ((!interleaved && !planar) || !input->mBuffers[0].mData || (planar && !input->mBuffers[1].mData)) return noErr;
    uint32_t frames = input->mBuffers[0].mDataByteSize / (sizeof(float) * (interleaved ? 2 : 1));
    if (planar) frames = MIN(frames, input->mBuffers[1].mDataByteSize / sizeof(float));
    uint32_t write = atomic_load_explicit(&ring->write, memory_order_relaxed);
    uint32_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
    frames = MIN(frames, KS_AUDIO_FRAMES - (write - read));
    const float *left = input->mBuffers[0].mData;
    const float *right = planar ? input->mBuffers[1].mData : left + 1;
    for (uint32_t i = 0; i < frames; ++i) {
        uint32_t index = ((write + i) & (KS_AUDIO_FRAMES - 1)) * 2;
        ring->samples[index] = left[i * (interleaved ? 2 : 1)];
        ring->samples[index + 1] = right[i * (interleaved ? 2 : 1)];
    }
    atomic_store_explicit(&ring->write, write + frames, memory_order_release);
    return noErr;
}

API_AVAILABLE(macos(14.2))
@interface KSAppAudio : NSObject {
@public
    AudioObjectID tap, aggregate;
    AudioDeviceIOProcID callback;
    KSAudioRing *ring;
}
@property KSAudioLog log;
@property(strong) AVAudioEngine *engine;
@property(strong) AVAudioSourceNode *source;
@property(strong) NSRunningApplication *application;
@property float level;
@property BOOL muted;
@property(copy) NSString *outputUID;
@property(copy) NSArray<NSNumber *> *processes;
@property(strong) CATapDescription *tapDescription;
@property double sampleRate;
- (void)stop;
@end
@implementation KSAppAudio
- (instancetype)init { if ((self = [super init])) { _level = 1; ring = calloc(1, sizeof(*ring)); if (!ring) return nil; atomic_init(&ring->gain, 1); atomic_init(&ring->read, 0); atomic_init(&ring->write, 0); } return self; }
- (void)stop {
    [_engine stop];
    if (callback) {
        AudioDeviceStop(aggregate, callback);
        OSStatus removed = AudioDeviceDestroyIOProcID(aggregate, callback);
        // Never free callback storage if the driver refuses to unregister it.
        // The OS reclaims this exceptional quarantine when KeySteer exits.
        if (removed != noErr) { if (_log) _log("Audio driver refused callback removal; its buffer was quarantined until exit"); ring = NULL; }
        callback = NULL;
    }
    _source = nil; _engine = nil;
    if (aggregate) { AudioHardwareDestroyAggregateDevice(aggregate); aggregate = 0; }
    if (tap) { AudioHardwareDestroyProcessTap(tap); tap = 0; }
    if (ring) { atomic_store(&ring->read, 0); atomic_store(&ring->write, 0); }
}
- (void)dealloc { [self stop]; free(ring); }
@end

API_AVAILABLE(macos(14.2))
static NSArray<NSNumber *> *AudioProcesses(pid_t pid) {
    NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
    NSString *bundle = app.bundleIdentifier;
    AudioObjectPropertyAddress address = Address(kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeGlobal, 0);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, NULL, &size) != noErr) return @[];
    AudioObjectID *ids = calloc(1, size); if (!ids) return @[];
    NSMutableArray *result = [NSMutableArray array];
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, ids) == noErr) {
        for (UInt32 i = 0; i < size / sizeof(*ids); ++i) {
            pid_t owner = 0; UInt32 bytes = sizeof(owner);
            if (Read(ids[i], kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal, 0, &bytes, &owner) != noErr || owner == getpid()) continue;
            NSString *candidate = StringProperty(ids[i], kAudioProcessPropertyBundleID) ?: [NSRunningApplication runningApplicationWithProcessIdentifier:owner].bundleIdentifier;
            if (owner == pid || (bundle.length && ([bundle isEqualToString:candidate] || [candidate hasPrefix:[bundle stringByAppendingString:@"."]]))) [result addObject:@(ids[i])];
        }
    }
    free(ids); return [result sortedArrayUsingSelector:@selector(compare:)];
}

API_AVAILABLE(macos(14.2))
static NSString *StartApp(KSAppAudio *route, NSArray<NSNumber *> *processes, NSDictionary *output) {
    CATapDescription *description = [[CATapDescription alloc] initStereoMixdownOfProcesses:processes];
    description.name = @"KeySteer application audio";
    [description setPrivate:YES];
    description.muteBehavior = CATapMutedWhenTapped;
    OSStatus status = AudioHardwareCreateProcessTap(description, &route->tap);
    if (status != noErr) return Failure(@"Cannot create application tap; allow System Audio Recording for KeySteer", status);
    AudioStreamBasicDescription stream = {0}; UInt32 bytes = sizeof(stream);
    status = Read(route->tap, kAudioTapPropertyFormat, kAudioObjectPropertyScopeGlobal, 0, &bytes, &stream);
    if (status != noErr || stream.mFormatID != kAudioFormatLinearPCM || !(stream.mFormatFlags & kAudioFormatFlagIsFloat) || stream.mBitsPerChannel != 32 || stream.mChannelsPerFrame != 2 || stream.mSampleRate <= 0) return @"Application tap does not provide supported Float32 stereo audio";
    NSString *uid = [@"com.keysteer.tap." stringByAppendingString:NSUUID.UUID.UUIDString];
    NSDictionary *config = @{
        @kAudioAggregateDeviceNameKey: @"KeySteer application audio",
        @kAudioAggregateDeviceUIDKey: uid,
        @kAudioAggregateDeviceIsPrivateKey: @YES,
        @kAudioAggregateDeviceTapAutoStartKey: @YES,
        @kAudioAggregateDeviceTapListKey: @[@{ @kAudioSubTapUIDKey: description.UUID.UUIDString,
                                              @kAudioSubTapDriftCompensationKey: @YES }]
    };
    status = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)config, &route->aggregate);
    if (status != noErr) return Failure(@"Cannot create private audio device", status);
    // The source format must match the aggregate's actual capture sample rate.
    Float64 rate = stream.mSampleRate;
    status = Write(route->aggregate, kAudioDevicePropertyNominalSampleRate,
                   kAudioObjectPropertyScopeGlobal, 0, sizeof(rate), &rate);
    if (status != noErr) return Failure(@"Cannot configure application audio sample rate", status);
    UInt32 bufferFrames = 256;
    Write(route->aggregate, kAudioDevicePropertyBufferFrameSize,
          kAudioObjectPropertyScopeGlobal, 0, sizeof(bufferFrames), &bufferFrames);
    route.engine = [[AVAudioEngine alloc] init];
    if (output) {
        AudioDeviceID device = [output[@"id"] unsignedIntValue];
        status = AudioUnitSetProperty(route.engine.outputNode.audioUnit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &device, sizeof(device));
        if (status != noErr) return Failure(@"Cannot select application output", status);
    }
    AVAudioFormat *format = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:stream.mSampleRate channels:2 interleaved:NO];
    KSAudioRing *ring = route->ring;
    route.source = [[AVAudioSourceNode alloc] initWithFormat:format renderBlock:^OSStatus(BOOL *silence, const AudioTimeStamp *time, AVAudioFrameCount frames, AudioBufferList *data) {
        (void)time;
        uint32_t read = atomic_load_explicit(&ring->read, memory_order_relaxed);
        uint32_t write = atomic_load_explicit(&ring->write, memory_order_acquire);
        // A device clock change must not accumulate seconds of stale audio.
        if (write - read > 4096) read = write - 2048;
        uint32_t ready = MIN(frames, write - read);
        float gain = atomic_load_explicit(&ring->gain, memory_order_relaxed);
        for (UInt32 b = 0; b < data->mNumberBuffers; ++b) {
            AudioBuffer *buffer = &data->mBuffers[b];
            if (!buffer->mData) continue;
            memset(buffer->mData, 0, buffer->mDataByteSize);
            if (b >= 2 || buffer->mNumberChannels != 1) continue;
            UInt32 count = MIN(ready, buffer->mDataByteSize / sizeof(float));
            float *dest = buffer->mData;
            for (UInt32 i = 0; i < count; ++i) dest[i] = ring->samples[((read + i) & (KS_AUDIO_FRAMES - 1)) * 2 + b] * gain;
        }
        atomic_store_explicit(&ring->read, read + ready, memory_order_release);
        *silence = ready == 0 || gain == 0;
        return noErr;
    }];
    [route.engine attachNode:route.source];
    [route.engine connect:route.source to:route.engine.mainMixerNode format:format];
    NSError *error = nil;
    if (![route.engine startAndReturnError:&error]) return error.localizedDescription ?: @"Cannot start application audio output";
    status = AudioDeviceCreateIOProcID(route->aggregate, Capture, ring, &route->callback);
    if (status == noErr) status = AudioDeviceStart(route->aggregate, route->callback);
    if (status != noErr) return Failure(@"Cannot start application tap; allow System Audio Recording for KeySteer", status);
    // Start first so macOS can present its permission prompt. Probe afterwards:
    // some Core Audio versions accept Start even when capture was denied.
    status = Write(route->tap, kAudioTapPropertyDescription,
                   kAudioObjectPropertyScopeGlobal, 0, sizeof(description), &description);
    if (status != noErr) return Failure(@"Allow System Audio Recording for KeySteer", status);
    route.processes = processes;
    route.tapDescription = description;
    route.sampleRate = stream.mSampleRate;
    return nil;
}

@interface KSAudioController : NSObject
@property(strong) NSMutableDictionary<NSNumber *, id> *routes;
@property double nextMaintenance;
@property KSAudioLog log;
@end
@implementation KSAudioController
- (instancetype)init { if ((self = [super init])) _routes = [NSMutableDictionary dictionary]; return self; }
@end

void *KSCreateAudioController(KSAudioLog log) {
    @autoreleasepool {
        @try {
            KSAudioController *owner = [[KSAudioController alloc] init];
            owner.log = log;
            return (__bridge_retained void *)owner;
        } @catch (NSException *exception) {
            if (log) log("Cannot create the native audio controller");
            return NULL;
        }
    }
}
void KSDestroyAudioController(void *value) {
    @autoreleasepool {
        if (!value) return;
        KSAudioController *owner = CFBridgingRelease(value);
        @try { [owner.routes removeAllObjects]; }
        @catch (NSException *exception) {
            if (owner.log) owner.log("Native audio cleanup failed");
        }
    }
}
// Avoid Clang's availability runtime (Rust links with -nodefaultlibs).
// Every tap entry is guarded by the OS version; the app still runs on 14.0.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
bool KSMaintainAudio(void *value) {
    @autoreleasepool {
        @try {
            KSAudioController *owner = (__bridge KSAudioController *)value;
            if (!owner) return false;
            if (!owner.routes.count) return false;
            double now = NSProcessInfo.processInfo.systemUptime;
            if (now < owner.nextMaintenance) return true;
            owner.nextMaintenance = now + .25;
            NSArray *outputs = Outputs();
            for (NSNumber *pid in owner.routes.allKeys) {
                KSAppAudio *route = owner.routes[pid];
                bool missing = route.outputUID != nil;
                for (NSDictionary *output in outputs) if ([route.outputUID isEqualToString:output[@"uid"]]) missing = false;
                AudioStreamBasicDescription stream = {0}; UInt32 bytes = sizeof(stream);
                bool invalidFormat = Read(route->tap, kAudioTapPropertyFormat, kAudioObjectPropertyScopeGlobal, 0, &bytes, &stream) != noErr || stream.mSampleRate != route.sampleRate || stream.mChannelsPerFrame != 2 || stream.mBitsPerChannel != 32 || !(stream.mFormatFlags & kAudioFormatFlagIsFloat);
                NSArray *processes = AudioProcesses(pid.intValue);
                bool updateFailed = false;
                if (processes.count && ![route.processes isEqualToArray:processes]) {
                    CATapDescription *description = route.tapDescription;
                    description.processes = processes;
                    updateFailed = Write(route->tap, kAudioTapPropertyDescription, kAudioObjectPropertyScopeGlobal, 0, sizeof(description), &description) != noErr;
                    if (!updateFailed) { route.tapDescription = description; route.processes = processes; }
                }
                // Restoring the original path is safer than leaving an app muted
                // when its process, output device or render engine disappears.
                if (!route.application || route.application.terminated || !processes.count || invalidFormat || updateFailed || missing || !route.engine.running) {
                    [route stop]; [owner.routes removeObjectForKey:pid];
                }
            }
            return owner.routes.count != 0;
        } @catch (NSException *exception) { return true; }
    }
}
uint64_t KSAudioProcessIdentity(int32_t pid) {
    @autoreleasepool {
        @try {
            NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
            if (!app || app.terminated || !app.launchDate) return 0;
            double date = app.launchDate.timeIntervalSince1970;
            uint64_t identity = 0; memcpy(&identity, &date, sizeof(identity)); return identity;
        } @catch (NSException *exception) { return 0; }
    }
}
bool KSChangeAudio(void *value, int32_t pid, uint64_t started, uint32_t action, char *buffer, size_t capacity) {
    @autoreleasepool {
        bool ok = false; NSString *message = @"Cannot create audio controller";
        @try {
            KSAudioController *owner = (__bridge KSAudioController *)value;
            if (owner && pid == 0) message = SystemChange(action, &ok);
            else if (owner && (started == 0 || KSAudioProcessIdentity(pid) != started)) message = @"Application audio target has expired";
            else if (owner) {
                if ([NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:(NSOperatingSystemVersion){14, 2, 0}]) {
                    KSMaintainAudio(value);
                    NSArray *processes = AudioProcesses(pid);
                    if (!processes.count) message = @"This application has no audio process yet";
                    else {
                        KSAppAudio *old = owner.routes[@(pid)];
                        float level = old ? old.level : 1; BOOL muted = old ? old.muted : NO;
                        NSString *uid = old.outputUID;
                        NSDictionary *output = nil;
                        if (action < 2) level = fminf(1, fmaxf(0, level + (action == 1 ? .01f : -.01f)));
                        if (action == 2) muted = !muted;
                        if (action >= 3) {
                            output = NextOutput(Outputs(), uid ?: StringProperty(DefaultOutput(), kAudioDevicePropertyDeviceUID), action == 3);
                            uid = output[@"uid"];
                        } else if (uid) {
                            for (NSDictionary *candidate in Outputs()) if ([candidate[@"uid"] isEqualToString:uid]) { output = candidate; break; }
                        }
                        if ((action >= 3 || uid) && !output) message = @"Selected audio output is unavailable";
                        else if (level >= 1 && !muted && !uid) {
                            [owner.routes removeObjectForKey:@(pid)]; ok = true; message = @"App volume 100%";
                        } else if (old && [old.processes isEqualToArray:processes] && ((old.outputUID == uid) || [old.outputUID isEqualToString:uid])) {
                            old.level = level; old.muted = muted; atomic_store(&old->ring->gain, muted ? 0 : level); ok = true;
                        } else if (!old && owner.routes.count >= 32) {
                            message = @"Too many application audio routes (maximum 32)";
                        } else {
                            KSAppAudio *next = [[KSAppAudio alloc] init];
                            if (!next) message = @"Cannot allocate application audio buffers";
                            else {
                                next.log = owner.log;
                                next.application = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
                                next.level = level; next.muted = muted; next.outputUID = uid;
                                atomic_store(&next->ring->gain, muted ? 0 : level);
                                NSString *failure = StartApp(next, processes, output);
                                if (failure) { [next stop]; message = failure; }
                                else { [old stop]; owner.routes[@(pid)] = next; ok = true; }
                            }
                        }
                        if (ok) message = action >= 3 ? [@"App output: " stringByAppendingString:output[@"name"]] : action == 2 ? (muted ? @"App muted" : @"App unmuted") : [NSString stringWithFormat:@"App volume %.0f%%%s", level * 100, muted ? " · muted" : ""];
                    }
                } else message = @"Application audio control requires macOS 14.2 or later";
            }
        } @catch (NSException *exception) { message = exception.reason ?: @"Core Audio rejected the operation"; ok = false; }
        if (buffer && capacity) { const char *text = message.UTF8String ?: "Audio error"; size_t length = MIN(strlen(text), capacity - 1); memcpy(buffer, text, length); buffer[length] = 0; }
        return ok;
    }
}

#pragma clang diagnostic pop
