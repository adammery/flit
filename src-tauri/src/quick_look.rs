//! Quick Look for attachments: the system preview panel Finder opens on
//! Space, shown by Flit for a file on disk.
//!
//! why the native panel and not `qlmanage -p`: that tool opens its window
//! in a separate process, behind Flit and without focus. This panel is
//! Flit's own — in front, and closed again by Space, as in Finder.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventType, NSWindowDelegate};
use objc2_foundation::{NSInteger, NSObjectProtocol, NSString, NSURL};
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};

/// The one file the panel shows.
struct Ivars {
    path: PathBuf,
    url: Retained<NSURL>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and PreviewSource
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct PreviewSource;

    unsafe impl NSObjectProtocol for PreviewSource {}

    unsafe impl QLPreviewPanelDataSource for PreviewSource {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn number_of_items(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            1
        }

        // why an NSURL as the item: QuickLookUI makes every file URL a
        // preview item, titled with the file's name.
        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _index: NSInteger,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            Some(ProtocolObject::from_retained(self.ivars().url.clone()))
        }
    }

    unsafe impl NSWindowDelegate for PreviewSource {}

    unsafe impl QLPreviewPanelDelegate for PreviewSource {
        // The panel handles Escape and its close button itself but leaves
        // Space to whoever opened it — here Space closes it, as in Finder.
        //
        // why no early `return`: the macro converts only the final value
        // to the Objective-C BOOL it hands back.
        #[unsafe(method(previewPanel:handleEvent:))]
        fn handle_event(&self, panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> bool {
            match (panel, event) {
                (Some(panel), Some(event)) if is_space(event) => {
                    panel.orderOut(None);
                    true
                }
                _ => false,
            }
        }
    }
);

/// A press of the Space bar. Only a KeyDown carries characters — asking
/// any other event for them raises an Objective-C exception.
fn is_space(event: &NSEvent) -> bool {
    event.r#type() == NSEventType::KeyDown
        && event
            .charactersIgnoringModifiers()
            .is_some_and(|chars| chars.to_string() == " ")
}

impl PreviewSource {
    fn new(mtm: MainThreadMarker, path: &Path) -> Retained<Self> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let this = mtm.alloc::<Self>().set_ivars(Ivars {
            path: path.to_path_buf(),
            url,
        });
        // SAFETY: NSObject's plain init, as for any NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// What the panel shows. why kept here: the panel holds its data source
    /// and delegate without retaining them, so something else must keep
    /// them alive while it is open — and both live on the main thread.
    static SHOWN: RefCell<Option<Retained<PreviewSource>>> = const { RefCell::new(None) };
}

/// Show `path` in the Quick Look panel — or close the panel when it is
/// already showing that file, so Space toggles it as in Finder.
pub fn toggle(mtm: MainThreadMarker, path: &Path) {
    // SAFETY: on the main thread, which the marker proves.
    let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
        return;
    };
    let same_file = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .is_some_and(|source| source.ivars().path == path)
    });
    if panel.isVisible() && same_file {
        panel.orderOut(None);
        return;
    }
    let source = PreviewSource::new(mtm, path);
    // SAFETY: `source` is kept alive in SHOWN below until the next file
    // replaces it — and by then the panel points at the new one.
    unsafe {
        panel.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        panel.setDelegate(Some(&source));
        panel.reloadData();
    }
    panel.makeKeyAndOrderFront(None);
    SHOWN.set(Some(source));
}
