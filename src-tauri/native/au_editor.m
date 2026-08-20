// Native AU editor windows. MUST be called on the main thread (AppKit).
//
// still_open_au_editor: tries the plugin's own Cocoa view
// (kAudioUnitProperty_CocoaUI), falls back to CoreAudioKit's AUGenericView
// (parameter sliders — also covers AUv3 units bridged through the v2 API).
// Returns a retained NSWindow*; windows hide on close (releasedWhenClosed=NO)
// so still_show_au_editor can bring them back, and still_close_au_editor
// releases them for good (before a chain rebuild destroys the AudioUnit).

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

void *still_open_au_editor(void *unitPtr, const char *title) {
  AudioUnit unit = (AudioUnit)unitPtr;
  NSView *view = nil;

  // 1) The plugin's custom Cocoa view, when it ships one.
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

  // 2) Generic parameter view (works for every unit, incl. bridged AUv3).
  if (!view) {
    AUGenericView *gv = [[AUGenericView alloc] initWithAudioUnit:unit];
    gv.showsExpertParameters = YES;
    view = gv;
  }
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
  [win setContentView:view];
  [win center];
  [win makeKeyAndOrderFront:nil];
  return (__bridge_retained void *)win;
}

void still_show_au_editor(void *winPtr) {
  NSWindow *win = (__bridge NSWindow *)winPtr;
  [win makeKeyAndOrderFront:nil];
}

void still_close_au_editor(void *winPtr) {
  NSWindow *win = (__bridge_transfer NSWindow *)winPtr;
  [win close];
  // ARC releases `win` here (transfer).
}
