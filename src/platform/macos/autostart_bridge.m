#import <Foundation/Foundation.h>
#import <ServiceManagement/ServiceManagement.h>

#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

static bool NmkMainAppLoginItemRegisteredStatus(SMAppServiceStatus status) {
    return status == SMAppServiceStatusEnabled ||
           status == SMAppServiceStatusRequiresApproval;
}

bool NmkMainAppLoginItemIsRegistered(void) {
    @autoreleasepool {
        return NmkMainAppLoginItemRegisteredStatus(SMAppService.mainAppService.status);
    }
}

char *NmkSetMainAppLoginItemEnabled(bool enabled) {
    @autoreleasepool {
        SMAppService *service = SMAppService.mainAppService;
        SMAppServiceStatus status = service.status;
        if (enabled && NmkMainAppLoginItemRegisteredStatus(status)) return NULL;
        if (!enabled && (status == SMAppServiceStatusNotRegistered ||
                         status == SMAppServiceStatusNotFound)) return NULL;

        NSError *error = nil;
        BOOL succeeded = enabled
            ? [service registerAndReturnError:&error]
            : [service unregisterAndReturnError:&error];
        if (succeeded) return NULL;

        NSString *message = error.localizedDescription ?: @"Service Management rejected the request";
        return strdup(message.UTF8String);
    }
}

void NmkFreeNativeString(char *value) {
    free(value);
}
