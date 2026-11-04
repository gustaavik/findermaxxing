// Quick Look needs a "controller" in the window's responder chain: an object
// answering acceptsPreviewPanelControl: / beginPreviewPanelControl: /
// endPreviewPanelControl:. Qt's content view (QNSView) does not, and cxx-qt
// cannot reach AppKit, so these methods are added to the view's class at
// runtime. Built without ARC (manual retain/release) so the rest of the C++
// sources need no Objective-C flags.

#include "presentation/cpp/quick_look.h"

#import <AppKit/AppKit.h>
#import <Quartz/Quartz.h>
#include <objc/runtime.h>

#include <QtGui/QGuiApplication>
#include <QtGui/QWindow>

@interface FMQuickLookSource : NSObject <QLPreviewPanelDataSource>
@property (nonatomic, retain) NSArray<NSURL*>* items;
@end

@implementation FMQuickLookSource
- (NSInteger)numberOfPreviewItemsInPreviewPanel:(QLPreviewPanel*)panel
{
    return static_cast<NSInteger>(self.items.count);
}

- (id<QLPreviewItem>)previewPanel:(QLPreviewPanel*)panel previewItemAtIndex:(NSInteger)index
{
    return self.items[static_cast<NSUInteger>(index)];
}

- (void)dealloc
{
    [_items release];
    [super dealloc];
}
@end

namespace {

FMQuickLookSource* sharedSource()
{
    static FMQuickLookSource* source = [[FMQuickLookSource alloc] init];
    return source;
}

BOOL acceptsPreviewPanelControl(id, SEL, QLPreviewPanel*)
{
    return YES;
}

void beginPreviewPanelControl(id, SEL, QLPreviewPanel* panel)
{
    panel.dataSource = sharedSource();
}

void endPreviewPanelControl(id, SEL, QLPreviewPanel* panel)
{
    panel.dataSource = nil;
}

// class_addMethod leaves existing implementations alone, so this is idempotent.
void becomePanelController(NSView* view)
{
    Class viewClass = [view class];
    class_addMethod(viewClass, @selector(acceptsPreviewPanelControl:), reinterpret_cast<IMP>(acceptsPreviewPanelControl), "c@:@");
    class_addMethod(viewClass, @selector(beginPreviewPanelControl:), reinterpret_cast<IMP>(beginPreviewPanelControl), "v@:@");
    class_addMethod(viewClass, @selector(endPreviewPanelControl:), reinterpret_cast<IMP>(endPreviewPanelControl), "v@:@");
}

NSArray<NSURL*>* toUrls(const rust::Vec<rust::String>& fileUrls)
{
    NSMutableArray<NSURL*>* urls = [NSMutableArray arrayWithCapacity:fileUrls.size()];
    for (const rust::String& fileUrl : fileUrls) {
        NSString* text = [[NSString alloc] initWithBytes:fileUrl.data() length:fileUrl.size() encoding:NSUTF8StringEncoding];
        NSURL* url = [NSURL URLWithString:text];
        [text release];
        if (url != nil) {
            [urls addObject:url];
        }
    }
    return urls;
}

bool isPanelOpen()
{
    return [QLPreviewPanel sharedPreviewPanelExists] && [[QLPreviewPanel sharedPreviewPanel] isVisible];
}

} // namespace

bool toggleQuickLook(const rust::Vec<rust::String>& fileUrls)
{
    @autoreleasepool {
        if (isPanelOpen()) {
            [[QLPreviewPanel sharedPreviewPanel] orderOut:nil];
            return true;
        }
        QWindow* window = QGuiApplication::focusWindow();
        NSArray<NSURL*>* urls = toUrls(fileUrls);
        if (window == nullptr || urls.count == 0) {
            return true;
        }
        NSView* view = reinterpret_cast<NSView*>(window->winId());
        becomePanelController(view);
        sharedSource().items = urls;
        QLPreviewPanel* panel = [QLPreviewPanel sharedPreviewPanel];
        [panel updateController];
        [panel reloadData];
        [panel makeKeyAndOrderFront:nil];
        return true;
    }
}

void refreshQuickLook(const rust::Vec<rust::String>& fileUrls)
{
    @autoreleasepool {
        if (!isPanelOpen()) {
            return;
        }
        NSArray<NSURL*>* urls = toUrls(fileUrls);
        if (urls.count > 0) {
            sharedSource().items = urls;
            [[QLPreviewPanel sharedPreviewPanel] reloadData];
        }
    }
}
