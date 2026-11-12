//! The display under the game's window, from UIKit: iPhone Duo's fold (`UIView`'s reserved
//! regions of the division kind, iOS 27.1) and whether upright layouts suit the device.
#![allow(unsafe_code)]

use horizon_client_input::window::{DisplayShape, Division};
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_core_foundation::CGRect;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// UIKit's `UIEdgeInsets`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

// SAFETY: the layout and encoding of UIKit's `UIEdgeInsets`: four `CGFloat`s (64-bit on every
// device this runs on), top, left, bottom, right.
unsafe impl objc2::encode::Encode for EdgeInsets {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "UIEdgeInsets",
        &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
    );
}

/// The view's safe area (`safeAreaInsets`): left, top, right, bottom, in points.
pub fn safe_area(window: &Window) -> [f64; 4] {
    let Ok(handle) = window.window_handle() else {
        return [0.0; 4];
    };
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return [0.0; 4];
    };
    let view = handle.ui_view.as_ptr().cast::<AnyObject>();
    // SAFETY: the window's view is alive while the window is, and this runs on the main thread
    // (winit's), where UIView may be asked; `safeAreaInsets` exists since iOS 11.
    let insets: EdgeInsets = unsafe { msg_send![&*view, safeAreaInsets] };
    [insets.left, insets.top, insets.right, insets.bottom]
}

/// `UIViewReservedRegionQueryOptionsIncludeInactive`: the fold while the device lies flat too,
/// as iPhone Duo keeps it.
const INCLUDE_INACTIVE: usize = 1;
/// `UIUserInterfaceIdiomPad`.
const IDIOM_PAD: isize = 1;

pub fn shape(window: &Window) -> DisplayShape {
    let Ok(handle) = window.window_handle() else {
        return DisplayShape::default();
    };
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return DisplayShape::default();
    };
    let view = handle.ui_view.as_ptr().cast::<AnyObject>();
    let scale = window.scale_factor();
    // SAFETY: the window's view is alive while the window is, winit hands it out on the main
    // thread, which runs this; every selector exists on the classes it is sent to (the reserved
    // region classes are looked up first: they are iOS 27.1's).
    let division = unsafe { division(&*view, scale) };
    // SAFETY: UIDevice is in every iOS; currentDevice and userInterfaceIdiom are main-thread safe.
    let idiom: isize = unsafe {
        let device: *mut AnyObject = msg_send![AnyClass::get(c"UIDevice").unwrap(), currentDevice];
        msg_send![device, userInterfaceIdiom]
    };
    DisplayShape {
        portrait: division.is_some() || idiom == IDIOM_PAD,
        division,
    }
}

/// # Safety
/// `view` is a live `UIView`, on the main thread.
unsafe fn division(view: &AnyObject, scale: f64) -> Option<Division> {
    let kind_class = AnyClass::get(c"UIViewReservedRegionKind")?;
    // SAFETY: the caller's.
    unsafe {
        let kind: *mut AnyObject = msg_send![kind_class, divisionRegionKind];
        let regions: *mut AnyObject =
            msg_send![view, reservedRegionsOfKind: kind, options: INCLUDE_INACTIVE];
        let count: usize = msg_send![regions, count];
        if count == 0 {
            return None;
        }
        let region: *mut AnyObject = msg_send![regions, objectAtIndex: 0usize];
        let frame: CGRect = msg_send![region, frame];
        let bounds: CGRect = msg_send![view, bounds];
        // A flat device's fold can be a line or a point: then the window's shape says which way
        // it runs (upright, the fold crosses the height).
        let horizontal = if frame.size.width.max(frame.size.height) > 1.0 {
            frame.size.width > frame.size.height
        } else {
            bounds.size.height > bounds.size.width
        };
        let (start, length) = if horizontal {
            (frame.origin.y, frame.size.height)
        } else {
            (frame.origin.x, frame.size.width)
        };
        // A point-sized fold sits in the middle of the window.
        let start = if frame.size.width.max(frame.size.height) > 1.0 {
            start
        } else if horizontal {
            bounds.size.height / 2.0
        } else {
            bounds.size.width / 2.0
        };
        Some(Division {
            horizontal,
            start: start * scale,
            end: (start + length) * scale,
        })
    }
}
