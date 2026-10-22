//! macOS: files opened from Finder (double-click, Open With, dropped on the Dock icon).
//!
//! Finder doesn't put them on the command line: it sends the running app (starting it first if
//! need be) an "open documents" Apple Event. winit's application delegate doesn't answer it, so
//! AppKit's default handler reported that CADaddy can't open the file (#158). [`install`]
//! registers a handler for the event that passes the files' paths to a callback; the app queues
//! them for the window, which opens them as tabs.
//!
//! AppKit installs its default handlers before `applicationWillFinishLaunching`, so this one is
//! installed when that notification arrives (Apple's "How Cocoa Applications Handle Apple
//! Events"), which is also before the event that started the app is delivered.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Files one event may name; the rest are ignored.
pub const MAX_FILES: usize = 256;

/// Where the files go: called on the main thread with their paths.
pub type Sink = Box<dyn Fn(Vec<String>) + 'static>;

/// Register the handler. Call once, on the main thread, before the event loop runs. Does nothing
/// off macOS.
#[cfg(not(target_os = "macos"))]
pub fn install(_sink: Sink) {}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(target_os = "macos")]
mod mac {
    use super::{MAX_FILES, Sink};
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter};

    /// Four-character Apple Event codes.
    const fn code(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }
    const CORE_EVENT_CLASS: u32 = code(b"aevt");
    const OPEN_DOCUMENTS: u32 = code(b"odoc");
    const DIRECT_OBJECT: u32 = code(b"----");
    const FILE_URL: u32 = code(b"furl");

    struct Ivars {
        sink: Sink,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = Ivars]
        struct OpenHandler;

        impl OpenHandler {
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                // SAFETY: `self` answers the selector with the signature the Apple Event Manager
                // calls (event, reply), and lives for the rest of the process (`install`).
                // The class and event ID are FourCharCodes (u32).
                let manager = NSAppleEventManager::sharedAppleEventManager();
                let handler = sel!(handleOpenDocuments:withReplyEvent:);
                let () = unsafe {
                    msg_send![&*manager, setEventHandler: self, andSelector: handler, forEventClass: CORE_EVENT_CLASS, andEventID: OPEN_DOCUMENTS]
                };
            }

            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                let files = paths(event);
                if !files.is_empty() {
                    (self.ivars().sink)(files);
                }
            }
        }
    );

    /// The paths an open-documents event names: its direct object, a list of file references
    /// (or one), each coerced to a file URL.
    fn paths(event: &NSAppleEventDescriptor) -> Vec<String> {
        // SAFETY: `paramDescriptorForKeyword:` takes an AEKeyword (u32) and returns a descriptor or nil.
        let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
        let Some(list) = list else { return Vec::new() };
        let count = usize::try_from(list.numberOfItems()).unwrap_or(0);
        let items: Vec<Retained<NSAppleEventDescriptor>> = if count == 0 {
            vec![list]
        } else {
            // Apple Event lists count from 1.
            (1..=count.min(MAX_FILES)).filter_map(|i| list.descriptorAtIndex(isize::try_from(i).ok()?)).collect()
        };
        items.iter().filter_map(|item| file_path(item)).collect()
    }

    fn file_path(item: &NSAppleEventDescriptor) -> Option<String> {
        // SAFETY: `coerceToDescriptorType:` takes a DescType (u32) and returns a descriptor or nil.
        let url: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![item, coerceToDescriptorType: FILE_URL] };
        let url = url?;
        // A file URL descriptor holds the URL's UTF-8 text.
        let uri = String::from_utf8_lossy(&url.data().to_vec()).into_owned();
        local_path(&uri)
    }

    /// `file:///Volumes/Projects/Sample%20Plan.dxf` → `/Volumes/Projects/Sample Plan.dxf`.
    fn local_path(uri: &str) -> Option<String> {
        let rest = uri.strip_prefix("file://")?;
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        let b = path.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while let Some(&c) = b.get(i) {
            let hex = |j: usize| b.get(j).and_then(|h| char::from(*h).to_digit(16));
            if c == b'%'
                && let (Some(hi), Some(lo)) = (hex(i + 1), hex(i + 2))
            {
                out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'?'));
                i += 3;
            } else {
                out.push(c);
                i += 1;
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    /// Register the handler (see the crate docs). Off the main thread it does nothing.
    pub fn install(sink: Sink) {
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!("files opened from Finder: not on the main thread");
            return;
        };
        let handler = OpenHandler::alloc(mtm).set_ivars(Ivars { sink });
        // SAFETY: NSObject's designated initialiser.
        let handler: Retained<OpenHandler> = unsafe { msg_send![super(handler), init] };
        // SAFETY: the selector is one `OpenHandler` answers, with a notification argument; the
        // notification name is AppKit's constant.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &handler,
                sel!(willFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
            );
        }
        // Neither the notification centre nor the Apple Event Manager keeps the handler alive;
        // it serves for the life of the process.
        std::mem::forget(handler);
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn file_urls_become_paths() {
            assert_eq!(super::local_path("file:///Volumes/Projects/Sample%20Plan.dxf").as_deref(), Some("/Volumes/Projects/Sample Plan.dxf"));
            assert_eq!(super::local_path("file://localhost/tmp/b.dwg").as_deref(), Some("/tmp/b.dwg"));
            assert_eq!(super::local_path("file://server/b.dwg"), None);
            assert_eq!(super::local_path("http://a/b.dwg"), None);
        }
    }
}
