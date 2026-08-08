#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>

typedef DWORD(WINAPI *WaitForCompositorClockFn)(UINT, const HANDLE *, DWORD);
typedef HRESULT(WINAPI *BoostCompositorClockFn)(BOOL);

static INIT_ONCE keysteer_clock_once = INIT_ONCE_STATIC_INIT;
// Retained for the process lifetime because the cached function pointers refer
// into this module. This is one system-DLL reference, not a per-gesture leak.
static HMODULE keysteer_dcomp_module = NULL;
static WaitForCompositorClockFn keysteer_wait_for_compositor_clock = NULL;
static BoostCompositorClockFn keysteer_boost_compositor_clock = NULL;

static BOOL CALLBACK keysteer_initialize_compositor_clock(
    PINIT_ONCE once,
    PVOID parameter,
    PVOID *context) {
    (void)once;
    (void)parameter;
    (void)context;

    keysteer_dcomp_module = LoadLibraryExW(
        L"dcomp.dll", NULL, LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (keysteer_dcomp_module == NULL) {
        return TRUE;
    }

    keysteer_wait_for_compositor_clock = (WaitForCompositorClockFn)GetProcAddress(
        keysteer_dcomp_module, "DCompositionWaitForCompositorClock");
    keysteer_boost_compositor_clock = (BoostCompositorClockFn)GetProcAddress(
        keysteer_dcomp_module, "DCompositionBoostCompositorClock");
    return TRUE;
}

static void keysteer_ensure_compositor_clock(void) {
    InitOnceExecuteOnce(
        &keysteer_clock_once, keysteer_initialize_compositor_clock, NULL, NULL);
}

// Returns zero when the Windows 11 compositor-clock API is unavailable.
intptr_t keysteer_compositor_clock_create(void) {
    keysteer_ensure_compositor_clock();
    if (keysteer_wait_for_compositor_clock == NULL) {
        return 0;
    }
    return (intptr_t)CreateEventW(NULL, TRUE, FALSE, NULL);
}

// 1 = compositor frame, 0 = stop event, -1 = unavailable/failure.
intptr_t keysteer_compositor_clock_wait(intptr_t stop_event) {
    if (keysteer_wait_for_compositor_clock == NULL || stop_event == 0) {
        return -1;
    }

    HANDLE handles[] = {(HANDLE)stop_event};
    DWORD result = keysteer_wait_for_compositor_clock(1, handles, INFINITE);
    if (result == WAIT_OBJECT_0 + 1) {
        return 1;
    }
    if (result == WAIT_OBJECT_0) {
        return 0;
    }
    return -1;
}

intptr_t keysteer_compositor_clock_signal(intptr_t stop_event) {
    if (stop_event == 0) {
        return 0;
    }
    return SetEvent((HANDLE)stop_event) ? 1 : 0;
}

intptr_t keysteer_compositor_clock_boost(intptr_t enable) {
    if (keysteer_boost_compositor_clock == NULL) {
        return 0;
    }
    return SUCCEEDED(keysteer_boost_compositor_clock(enable != 0)) ? 1 : 0;
}
