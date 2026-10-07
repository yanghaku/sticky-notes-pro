//! Native Windows borders without a system caption above the note toolbar.
//!
//! Keep the native side/bottom frame and a one-physical-pixel top edge. Extend
//! the client area into the remaining top resize band, compensating winit's
//! default frame-size calculations without changing interactive resize sizes.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::{io, ptr};

use anyhow::{Context, Result, ensure};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow},
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
            HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsIconic, IsZoomed,
            MINMAXINFO, NCCALCSIZE_PARAMS, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongW, SetWindowPos, WINDOWPOS,
            WM_DPICHANGED, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST,
            WM_STYLECHANGING, WM_WINDOWPOSCHANGING, WS_BORDER, WS_CAPTION, WS_THICKFRAME,
        },
    },
};

use super::winit;

const SUBCLASS_ID: usize = 1;
const TOP_BORDER: i32 = 1;
// Subclass ref-data contains only flags, never a pointer or a Rust reference.
const CLIENT_RESIZE: usize = 1;
const DPI_RESIZE: usize = 2;

/// Mark only synchronous application/backend requests, not arbitrary native
/// size-only calls. Keep the window alive and clear the marker even on unwind.
pub(super) fn with_client_resize<T>(
    window: &winit::window::Window,
    update: impl FnOnce() -> T,
) -> T {
    let _guard = ClientResizeGuard::new(window);
    update()
}

struct ClientResizeGuard<'a> {
    _window: &'a winit::window::Window,
    hwnd: HWND,
    previously_marked: bool,
}

impl<'a> ClientResizeGuard<'a> {
    fn new(window: &'a winit::window::Window) -> Option<Self> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get() as HWND;
        // SAFETY: winit owns this live HWND. No pointers are retained by the
        // queries; only its owner thread may access the subclass marker.
        if unsafe { GetWindowThreadProcessId(hwnd, ptr::null_mut()) != GetCurrentThreadId() } {
            return None;
        }
        let flags = frame_flags(hwnd)?;
        if !set_frame_flags(hwnd, flags | CLIENT_RESIZE) {
            return None;
        }
        Some(Self {
            _window: window,
            hwnd,
            previously_marked: flags & CLIENT_RESIZE != 0,
        })
    }
}

impl Drop for ClientResizeGuard<'_> {
    fn drop(&mut self) {
        if let Some(flags) = frame_flags(self.hwnd) {
            // Preserve independent flags changed by nested Windows messages.
            let restored = if self.previously_marked {
                flags | CLIENT_RESIZE
            } else {
                flags & !CLIENT_RESIZE
            };
            set_frame_flags(self.hwnd, restored);
        }
    }
}

pub(super) fn install(window: &winit::window::Window) -> Result<()> {
    use winit::platform::windows::{Color, WindowExtWindows};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    ensure!(
        window.is_decorated(),
        "native note frame requires window decorations"
    );
    let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
        anyhow::bail!("note window is not a Win32 window");
    };
    let hwnd = handle.hwnd.get() as HWND;
    // SAFETY: winit owns this live HWND. Neither query retains pointers; the
    // null process-ID output is allowed. Subclassing must stay on its UI thread.
    let (owner, current) = unsafe {
        (
            GetWindowThreadProcessId(hwnd, ptr::null_mut()),
            GetCurrentThreadId(),
        )
    };
    ensure!(
        owner == current,
        "native note frame must be installed on its window thread"
    );
    let client_size = window.inner_size();
    // SAFETY: HWND is live on this thread. GWL_STYLE is a 32-bit bitmask (not a
    // pointer), so GetWindowLongW works on both 32- and 64-bit Windows.
    let original_style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
    // SAFETY: callback has the SUBCLASSPROC ABI, stores no Rust references and
    // remains valid for the process lifetime. The owner-thread check is above.
    ensure!(
        unsafe { SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, 0) } != 0,
        "cannot install native note frame subclass"
    );
    if let Err(error) = write_style(hwnd, without_caption(original_style as u32) as i32)
        .and_then(|()| refresh_frame(hwnd))
    {
        // SAFETY: remove exactly our callback before restoring the style; HWND
        // is still owned by winit on this same thread. No ref-data needs freeing.
        unsafe { RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID) };
        if let Err(rollback) = write_style(hwnd, original_style).and_then(|()| refresh_frame(hwnd))
        {
            eprintln!("cannot restore native note frame after setup failure: {rollback:#}");
        }
        return Err(error);
    }
    // None would HIDE the border. Leave both color and light/dark appearance to
    // Windows, including its live setting changes; do not force window.set_theme.
    // This DWM attribute is ignored on pre-Windows-11 systems.
    window.set_border_color(Some(Color::SYSTEM_DEFAULT));
    // Adding a native frame changes the client area, not the saved note size.
    // WM_WINDOWPOSCHANGING also removes the now-unused top inset from winit's
    // requested outer height, preserving this client size exactly.
    with_client_resize(window, || {
        let _ = window.request_inner_size(client_size);
    });
    Ok(())
}

fn without_caption(style: u32) -> u32 {
    // WS_CAPTION includes WS_BORDER. Restore that bit, retaining WS_THICKFRAME
    // and every unrelated bit. Leave a genuinely borderless fullscreen style
    // untouched rather than reintroducing borders if fullscreen is added later.
    if style & (WS_CAPTION | WS_THICKFRAME) == 0 {
        style
    } else {
        (style & !WS_CAPTION) | WS_BORDER
    }
}

fn write_style(hwnd: HWND, style: i32) -> Result<()> {
    // SAFETY: the caller supplies a live owner-thread HWND; this only writes a
    // 32-bit style, never a function pointer or window-owned user data.
    let (previous, error) = unsafe {
        SetLastError(0);
        let previous = SetWindowLongW(hwnd, GWL_STYLE, style);
        (previous, GetLastError())
    };
    if previous == 0 && error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32))
            .context("cannot change note frame style");
    }
    Ok(())
}

fn refresh_frame(hwnd: HWND) -> Result<()> {
    // SAFETY: HWND is live on this thread, the ignored rectangle values and
    // null insertion handle are valid with NOSIZE/NOMOVE/NOZORDER flags.
    let success = unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    ensure!(
        success != 0,
        "cannot refresh note frame: {}",
        io::Error::last_os_error()
    );
    Ok(())
}

fn normal_frame(hwnd: HWND) -> Option<RECT> {
    // SAFETY: all callers run on the live HWND's owner thread. Queries retain
    // no pointers, and AdjustWindowRectExForDpi writes only this local RECT.
    let (style, ex_style, dpi) = unsafe {
        (
            GetWindowLongW(hwnd, GWL_STYLE) as u32,
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            GetDpiForWindow(hwnd),
        )
    };
    if style & WS_THICKFRAME == 0 || dpi == 0 {
        return None;
    }
    let mut frame = RECT::default();
    // SAFETY: frame is writable; these are this window's current style/DPI.
    // The note has no native menu (its menu is a Slint popup).
    if unsafe { AdjustWindowRectExForDpi(&mut frame, style, 0, ex_style, dpi) } == 0 {
        return None;
    }
    Some(frame)
}

fn normal_window(hwnd: HWND) -> bool {
    // SAFETY: these queries operate on the live HWND without retaining data.
    unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 }
}

fn removed_top(frame: &RECT) -> i32 {
    frame.top.saturating_neg().saturating_sub(TOP_BORDER).max(0)
}

fn frame_flags(hwnd: HWND) -> Option<usize> {
    let mut flags = 0;
    // SAFETY: HWND belongs to the caller's UI thread; this queries only our
    // callback/id and writes a scalar into the local output. No data is retained.
    let found = unsafe { GetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, &mut flags) };
    (found != 0).then_some(flags)
}

fn set_frame_flags(hwnd: HWND, flags: usize) -> bool {
    // SAFETY: update only our existing subclass on the window's owner thread;
    // flags are scalar ref-data, with no allocation or lifetime to manage.
    if unsafe { SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, flags) } == 0 {
        eprintln!("cannot update native note frame flags");
        return false;
    }
    true
}

fn top_hit_test(hwnd: HWND, lparam: LPARAM, default_hit: LRESULT) -> LRESULT {
    let client_or_side = default_hit == HTCLIENT as LRESULT
        || default_hit == HTLEFT as LRESULT
        || default_hit == HTRIGHT as LRESULT;
    if !client_or_side || !normal_window(hwnd) {
        return default_hit;
    }
    let Some(frame) = normal_frame(hwnd) else {
        return default_hit;
    };
    let mut bounds = RECT::default();
    // SAFETY: HWND is live and bounds is a writable local RECT.
    if unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return default_hit;
    }
    // WM_NCHITTEST packs signed screen coordinates, including negative ones
    // on monitors to the left/above the primary monitor.
    let x = i32::from(lparam as i16);
    let y = i32::from((lparam >> 16) as i16);
    let height = frame.top.saturating_neg().max(TOP_BORDER);
    if x < bounds.left
        || x >= bounds.right
        || y < bounds.top
        || y >= bounds.top.saturating_add(height)
    {
        return default_hit;
    }
    let width = frame.left.saturating_neg().max(frame.right).max(1);
    if x < bounds.left.saturating_add(width) {
        HTTOPLEFT as LRESULT
    } else if x >= bounds.right.saturating_sub(width) {
        HTTOPRIGHT as LRESULT
    } else {
        HTTOP as LRESULT
    }
}

/// # Safety
/// Called only by comctl32 for our installed subclass on the HWND's UI thread;
/// message parameters must obey the Windows message-specific pointer contracts.
unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_DPICHANGED {
        // winit owns DPI events and logical-size preservation. Its nested
        // SetWindowPos still uses the default frame inset, so compensate that
        // resize even when this DPI change happens during a native drag.
        set_frame_flags(hwnd, data | DPI_RESIZE);
        // SAFETY: forward the unchanged DPI message synchronously; no message
        // parameter is borrowed across the call or stored in ref-data.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        set_frame_flags(hwnd, data);
        return result;
    }
    if message == WM_NCCALCSIZE && lparam != 0 {
        let rect = if wparam != 0 {
            // SAFETY: with nonzero wParam, Windows supplies NCCALCSIZE_PARAMS.
            // Keep only a raw pointer across forwarding, not a Rust reference.
            unsafe { ptr::addr_of_mut!((*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]) }
        } else {
            lparam as *mut RECT
        };
        // SAFETY: either documented form supplies a valid first RECT.
        let outer_top = unsafe { (*rect).top };
        // SAFETY: let Windows calculate side/bottom and maximized insets first.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if normal_window(hwnd) && normal_frame(hwnd).is_some() {
            // SAFETY: the synchronous message's RECT remains writable. Retain
            // the thin native edge while exposing its thick resize band to UI.
            unsafe { (*rect).top = outer_top.saturating_add(TOP_BORDER).min((*rect).bottom) };
            // The default valid-source rectangles were computed for a different
            // top inset. Do not reuse them after changing the client rectangle.
            return 0;
        }
        // Maximized windows retain Windows' normal work-area clipping, including
        // its offscreen resize border; never expose that area to the note UI.
        return result;
    }
    if message == WM_NCHITTEST {
        // SAFETY: preserve Windows' hit tests for the other native edges.
        let hit = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        return top_hit_test(hwnd, lparam, hit);
    }
    if message == WM_GETMINMAXINFO && lparam != 0 {
        let info = lparam as *mut MINMAXINFO;
        // SAFETY: this message supplies a live MINMAXINFO. Copy fields before
        // forwarding rather than keeping a Rust borrow across reentrant calls.
        let (min_height, max_height) =
            unsafe { ((*info).ptMinTrackSize.y, (*info).ptMaxTrackSize.y) };
        // SAFETY: winit applies the Slint client-size constraints here.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if let Some(frame) = normal_frame(hwnd) {
            let removed = removed_top(&frame);
            // SAFETY: MINMAXINFO remains writable until this message returns.
            // Only alter track limits changed by winit, not OS maximize geometry.
            unsafe {
                if (*info).ptMinTrackSize.y != min_height {
                    (*info).ptMinTrackSize.y =
                        (*info).ptMinTrackSize.y.saturating_sub(removed).max(1);
                }
                if (*info).ptMaxTrackSize.y != max_height {
                    (*info).ptMaxTrackSize.y =
                        (*info).ptMaxTrackSize.y.saturating_sub(removed).max(1);
                }
            }
        }
        return result;
    }
    if message == WM_WINDOWPOSCHANGING && lparam != 0 && normal_window(hwnd) {
        // SAFETY: Windows supplies a writable WINDOWPOS for this message; the
        // borrow ends before the final DefSubclassProc call.
        let position = unsafe { &mut *(lparam as *mut WINDOWPOS) };
        // The explicit scope identifies our client-size requests. Flags only
        // exclude moves/activation (e.g. maximize/restore within a property
        // update); they must never be used alone to infer the caller.
        let size_flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        let client_resize = data & CLIENT_RESIZE != 0 && position.flags & size_flags == size_flags;
        if position.flags & (SWP_NOSIZE | SWP_FRAMECHANGED) == 0
            && (client_resize || data & DPI_RESIZE != 0)
            && let Some(frame) = normal_frame(hwnd)
        {
            position.cy = position.cy.saturating_sub(removed_top(&frame)).max(1);
        }
    }
    if message == WM_STYLECHANGING && wparam as i32 == GWL_STYLE && lparam != 0 {
        // SAFETY: WM_STYLECHANGING supplies a writable STYLESTRUCT for this
        // synchronous call. End the borrow before forwarding/reentrant FFI.
        let styles = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
        styles.styleNew = without_caption(styles.styleNew);
    }
    if message == WM_NCDESTROY {
        // SAFETY: still inside destruction of the live HWND; remove our exact
        // callback/id pair before forwarding. There is no heap-owned ref-data.
        unsafe { RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID) };
    }
    // SAFETY: forward the original message unchanged through the subclass chain.
    // winit must continue receiving focus, IME, size, DPI and destroy messages.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
