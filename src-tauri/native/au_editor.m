// Native AU editor windows. MUST be called on the main thread (AppKit).
//
// still_open_au_editor: tries the plugin's own Cocoa view
// (kAudioUnitProperty_CocoaUI), falls back to CoreAudioKit's AUGenericView.
// A property listener on kAudioUnitProperty_ClassInfo REBUILDS the view in
// place when the plugin swaps its configuration (preset loads from the
// plugin's own UI — iZotope et al. expect the host to do this; keeping the
// old NSView leaves a dead, unresponsive interface).
//
// The opaque handle returned to Rust is a retained StillEditorContext.

#import <Cocoa/Cocoa.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudioKit/CoreAudioKit.h>

// The AUv2 Cocoa view factory protocol (historically in
// <AudioUnit/AUCocoaUIView.h>, absent from the modular headers).
@protocol StillAUCocoaUIBase <NSObject>
- (unsigned)interfaceVersion;
- (NSView *)uiViewForAudioUnit:(AudioUnit)inAudioUnit
                      withSize:(NSSize)inPreferredSize;
@end

@interface StillEditorContext : NSObject
@property(nonatomic, assign) AudioUnit unit;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, assign) int64_t generation;
@end
@implementation StillEditorContext
@end

static NSView *still_build_view(AudioUnit unit) {
  NSView *view = nil;
  UInt32 size = 0;
  Boolean writable = false;
  if (AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_CocoaUI,
                               kAudioUnitScope_Global, 0, &size,
                               &writable) == noErr &&
      size >= sizeof(AudioUnitCocoaViewInfo)) {
    AudioUnitCocoaViewInfo *info = (AudioUnitCocoaViewInfo *)malloc(size);
    if (AudioUnitGetProperty(unit, kAudioUnitProperty_CocoaUI,
                             kAudioUnitScope_Global, 0, info, &size) == noErr) {
      NSURL *bundleURL = (__bridge NSURL *)info->mCocoaAUViewBundleLocation;
      NSString *className = (__bridge NSString *)info->mCocoaAUViewClass[0];
      NSBundle *bundle = [NSBundle bundleWithURL:bundleURL];
      Class factoryClass = bundle ? [bundle classNamed:className] : nil;
      id<StillAUCocoaUIBase> factory =
          factoryClass ? [[factoryClass alloc] init] : nil;
      if (factory &&
          [factory respondsToSelector:@selector(uiViewForAudioUnit:withSize:)]) {
        @try {
          view = [factory uiViewForAudioUnit:unit
                                    withSize:NSMakeSize(640, 420)];
        } @catch (NSException *e) {
          view = nil;
        }
      }
      UInt32 classCount =
          (size - sizeof(CFURLRef)) / (UInt32)sizeof(CFStringRef);
      if (info->mCocoaAUViewBundleLocation)
        CFRelease(info->mCocoaAUViewBundleLocation);
      for (UInt32 i = 0; i < classCount; i++)
        if (info->mCocoaAUViewClass[i])
          CFRelease(info->mCocoaAUViewClass[i]);
    }
    free(info);
  }
  if (!view) {
    AUGenericView *gv = [[AUGenericView alloc] initWithAudioUnit:unit];
    gv.showsExpertParameters = YES;
    view = gv;
  }
  return view;
}

static void still_mount_view(NSWindow *win, NSView *view);

// The plugin changed its whole configuration (preset load): rebuild the
// view. Debounced — notifications can burst during one load, and the plugin
// may not be ready for a view request immediately.
static void still_classinfo_changed(void *refcon, AudioUnit unit,
                                    AudioUnitPropertyID prop,
                                    AudioUnitScope scope,
                                    AudioUnitElement elem) {
  (void)prop;
  (void)scope;
  (void)elem;
  StillEditorContext *ctx = (__bridge StillEditorContext *)refcon;
  dispatch_async(dispatch_get_main_queue(), ^{
    int64_t gen = ++ctx.generation;
    dispatch_after(
        dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.4 * NSEC_PER_SEC)),
        dispatch_get_main_queue(), ^{
          if (ctx.generation != gen || !ctx.window)
            return;
          NSView *fresh = still_build_view(unit);
          if (fresh) {
            NSRect f = fresh.frame;
            if (f.size.width > 100 && f.size.height > 60)
              [ctx.window setContentSize:f.size];
            still_mount_view(ctx.window, fresh);
          }
        });
  });
}

// Embed the AU view inside a CONTAINER — never directly as contentView.
// Plugins that rebuild their UI (iZotope's Hook/Core architecture reloads
// its whole core on preset changes) replace their view within its
// superview; with the view as contentView that replacement fails and
// leaves a dead interface. Inside a plain container it just works.
static void still_mount_view(NSWindow *win, NSView *view) {
  NSView *container = win.contentView;
  for (NSView *sub in [container.subviews copy])
    [sub removeFromSuperview];
  view.frame = container.bounds;
  view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
  view.postsFrameChangedNotifications = YES;
  [container addSubview:view];
}

void *still_open_au_editor(void *unitPtr, const char *title) {
  AudioUnit unit = (AudioUnit)unitPtr;
  NSView *view = still_build_view(unit);
  if (!view)
    return NULL;

  NSRect frame = view.frame;
  if (frame.size.width < 240 || frame.size.height < 120)
    frame.size = NSMakeSize(560, 420);

  NSWindow *win = [[NSWindow alloc]
      initWithContentRect:NSMakeRect(0, 0, frame.size.width, frame.size.height)
                styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                           NSWindowStyleMaskResizable |
                           NSWindowStyleMaskMiniaturizable)
                  backing:NSBackingStoreBuffered
                    defer:NO];
  win.title = [NSString stringWithUTF8String:title];
  win.releasedWhenClosed = NO;
  win.contentView = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, frame.size.width,
                                                             frame.size.height)];
  still_mount_view(win, view);
  // Follow the plugin view's own size changes (preset loads may resize it).
  [[NSNotificationCenter defaultCenter]
      addObserverForName:NSViewFrameDidChangeNotification
                  object:view
                   queue:[NSOperationQueue mainQueue]
              usingBlock:^(NSNotification *note) {
                NSView *v = note.object;
                if (v.window == win && v.superview == win.contentView) {
                  NSSize s = v.frame.size;
                  if (s.width > 100 && s.height > 60 &&
                      !NSEqualSizes(s, [win.contentView frame].size))
                    [win setContentSize:s];
                }
              }];
  [win center];
  [win makeKeyAndOrderFront:nil];

  StillEditorContext *ctx = [[StillEditorContext alloc] init];
  ctx.unit = unit;
  ctx.window = win;
  ctx.generation = 0;
  AudioUnitAddPropertyListener(unit, kAudioUnitProperty_ClassInfo,
                               still_classinfo_changed, (__bridge void *)ctx);
  return (__bridge_retained void *)ctx;
}

void still_show_au_editor(void *ctxPtr) {
  StillEditorContext *ctx = (__bridge StillEditorContext *)ctxPtr;
  [ctx.window makeKeyAndOrderFront:nil];
}

void still_close_au_editor(void *ctxPtr) {
  StillEditorContext *ctx = (__bridge_transfer StillEditorContext *)ctxPtr;
  AudioUnitRemovePropertyListenerWithUserData(ctx.unit,
                                              kAudioUnitProperty_ClassInfo,
                                              still_classinfo_changed,
                                              (__bridge void *)ctx);
  [ctx.window close];
  ctx.window = nil;
  // ARC releases ctx here (transfer).
}
