// Sparkle 2 without linking it: the framework comes with the packaged app
// (Contents/Frameworks/Sparkle.framework), is loaded on first use, and is
// driven through the Objective-C runtime. Its feed URL and EdDSA public key
// come from Info.plist (SUFeedURL, SUPublicEDKey); with no feed URL (a
// development build) Sparkle stays off. Built without ARC, like the Quick
// Look shim: the controller is retained for the life of the process.

#include "presentation/cpp/updater.h"

#import <Foundation/Foundation.h>
#include <objc/message.h>

namespace {

constexpr auto kFrameworkName = @"Sparkle.framework";
constexpr auto kControllerClass = @"SPUStandardUpdaterController";
constexpr auto kFeedKey = @"SUFeedURL";

id controller = nil;

id updater()
{
    return controller == nil ? nil : [controller valueForKey:@"updater"];
}

} // namespace

bool startSparkle()
{
    if (controller != nil) {
        return true;
    }
    if ([[NSBundle mainBundle] objectForInfoDictionaryKey:kFeedKey] == nil) {
        return false;
    }
    NSString* path = [[[NSBundle mainBundle] privateFrameworksPath] stringByAppendingPathComponent:kFrameworkName];
    NSBundle* framework = [NSBundle bundleWithPath:path];
    if (framework == nil || ![framework load]) {
        return false;
    }
    Class type = NSClassFromString(kControllerClass);
    if (type == nil) {
        return false;
    }
    // -[SPUStandardUpdaterController initWithStartingUpdater:updaterDelegate:userDriverDelegate:]
    using Init = id (*)(id, SEL, BOOL, id, id);
    auto init = reinterpret_cast<Init>(objc_msgSend);
    controller = init([type alloc], @selector(initWithStartingUpdater:updaterDelegate:userDriverDelegate:), YES, nil, nil);
    return controller != nil;
}

void checkForUpdatesNow()
{
    if (startSparkle()) {
        using Action = void (*)(id, SEL, id);
        reinterpret_cast<Action>(objc_msgSend)(controller, @selector(checkForUpdates:), nil);
    }
}

bool automaticUpdateChecks()
{
    id current = updater();
    return current != nil && [[current valueForKey:@"automaticallyChecksForUpdates"] boolValue];
}

void setAutomaticUpdateChecks(bool enabled)
{
    [updater() setValue:@(enabled) forKey:@"automaticallyChecksForUpdates"];
}
