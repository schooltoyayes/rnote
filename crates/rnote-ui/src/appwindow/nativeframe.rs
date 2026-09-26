//! A native window frame on Windows.
//!
//! GTK draws the whole frame of the window itself on Windows, and tells the system that the window can
//! neither be resized nor maximized. Because of that the Windows snap features (Win + arrow keys,
//! dragging the window to the screen edges, the snap layouts) don't work, and the window can only be
//! resized through a thin invisible border that doesn't react to touch or pen input.
//!
//! This gives the window the styles of a normal window and handles the window messages that are needed
//! for that in a message filter on the GDK display:
//! - The system frame stays on the left, right and bottom, where it is the invisible resize border that
//!   every Windows app has. The title bar and the top frame are removed, so the header bars stay on
//!   top. The top resize border lies in the header bar instead.
//! - The empty parts of the header bars are reported as the title bar, so moving the window goes
//!   through the system and gets snapping.
//! - GTK is told that the window has system decorations, else it sizes the window back whenever the
//!   system resizes it.
//! - GTK computes the window size from the size of the content with the system title bar and frame.
//!   Its positions are corrected for the missing title bar.

// Imports
use adw::prelude::*;
use gtk4::glib::translate::ToGlibPtr;
use gtk4::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use tracing::{debug, error};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ScreenToClient,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HTBOTTOM,
    HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTMAXBUTTON, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, ISMEX_CALLBACK, ISMEX_NOTIFY, ISMEX_SEND, InSendMessageEx, IsIconic,
    IsZoomed, MSG, NCCALCSIZE_PARAMS, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WINDOWPOS,
    WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
    WM_STYLECHANGING, WM_WINDOWPOSCHANGING, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
    WS_SYSMENU, WS_THICKFRAME,
};

/// The styles of a normal window. These are also the ones GTK wants for windows with system decorations,
/// so it doesn't try to change them.
const FRAME_STYLES: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
/// Set by the system when a window is minimized, maximized or restored. Not in the headers.
const SWP_STATECHANGED: u32 = 0x8000;

const GDK_WIN32_MESSAGE_FILTER_CONTINUE: c_int = 0;
const GDK_WIN32_MESSAGE_FILTER_REMOVE: c_int = 1;

type MessageFilter = unsafe extern "C" fn(
    display: *mut c_void,
    msg: *mut MSG,
    return_value: *mut c_int,
    data: glib::ffi::gpointer,
) -> c_int;

#[link(name = "gtk-4")]
unsafe extern "C" {
    fn gdk_win32_display_add_filter(
        display: *mut c_void,
        function: MessageFilter,
        data: glib::ffi::gpointer,
    );
    fn gdk_win32_surface_get_handle(surface: *mut gdk::ffi::GdkSurface) -> HWND;
}

#[derive(Debug)]
struct Frame {
    window: glib::WeakRef<gtk4::Window>,
    /// The system is moving or resizing the window.
    in_size_move: Cell<bool>,
    /// The last style change was changed back to keep the frame.
    style_kept: Cell<bool>,
    /// The frame is applied, positions are not from GTK.
    applying: Cell<bool>,
    /// The maximize button of the header bar found by the last hit test.
    ///
    /// It is reported to the system as its maximize button, which shows the snap layouts when hovering
    /// it. GTK doesn't get the pointer events over it then, so they are passed on here.
    maximize_button: RefCell<Option<gtk4::Widget>>,
    maximize_button_hovered: Cell<bool>,
    maximize_button_pressed: Cell<bool>,
}

/// What a point of the content is for the system.
enum Area {
    Content,
    TitleBar,
    MaximizeButton(gtk4::Widget),
}

thread_local! {
    static FRAMES: RefCell<HashMap<isize, Rc<Frame>>> = RefCell::new(HashMap::new());
    static FILTER_INSTALLED: Cell<bool> = const { Cell::new(false) };
    static CSS_ADDED: Cell<bool> = const { Cell::new(false) };
}

/// Give the window a native frame. Call before the window is realized.
pub(crate) fn setup(window: &impl IsA<gtk4::Window>) {
    let window = window.upcast_ref::<gtk4::Window>();

    // GTK keeps a transparent margin around decorated windows for its own shadow and resize handles.
    // The system draws both now, and the margin would leave a gap around snapped windows.
    window.set_decorated(false);
    window.add_css_class("native-frame");
    add_css(window);

    window.connect_realize(attach);
    window.connect_unrealize(detach);

    let style_manager = adw::StyleManager::default();
    style_manager.connect_dark_notify(glib::clone!(
        #[weak]
        window,
        move |style_manager| {
            if let Some(hwnd) = hwnd(&window) {
                set_dark_mode(hwnd, style_manager.is_dark());
            }
        }
    ));
}

fn add_css(window: &gtk4::Window) {
    if CSS_ADDED.replace(true) {
        return;
    }
    let css = gtk4::CssProvider::new();
    // The system rounds the corners, so the window itself must not.
    css.load_from_string(
        "window.native-frame, window.native-frame.csd {
            border-radius: 0;
            box-shadow: none;
        }",
    );
    gtk4::style_context_add_provider_for_display(
        &WidgetExt::display(window),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn hwnd(window: &gtk4::Window) -> Option<HWND> {
    let surface = window.surface()?;
    let hwnd = unsafe { gdk_win32_surface_get_handle(surface.to_glib_none().0) };
    (!hwnd.is_null()).then_some(hwnd)
}

fn attach(window: &gtk4::Window) {
    let Some(hwnd) = hwnd(window) else {
        error!("Could not get the native window handle, keeping the GTK frame.");
        return;
    };
    install_filter(&WidgetExt::display(window));

    let frame = Rc::new(Frame {
        window: window.downgrade(),
        in_size_move: Cell::new(false),
        style_kept: Cell::new(false),
        applying: Cell::new(true),
        maximize_button: RefCell::new(None),
        maximize_button_hovered: Cell::new(false),
        maximize_button_pressed: Cell::new(false),
    });
    FRAMES.with(|frames| frames.borrow_mut().insert(hwnd as isize, Rc::clone(&frame)));

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, (style | FRAME_STYLES) as isize);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER
                | SWP_NOOWNERZORDER
                | SWP_NOACTIVATE,
        );
        let corners = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corners).cast(),
            size_of_val(&corners) as u32,
        );
    }
    // GTK takes the size the system gives the window only for windows with system decorations, else it
    // sizes the window back after snapping or maximizing.
    if let Some(toplevel) = window
        .surface()
        .and_then(|surface| surface.downcast::<gdk::Toplevel>().ok())
    {
        toplevel.set_decorated(true);
    }
    frame.applying.set(false);
    set_dark_mode(hwnd, adw::StyleManager::default().is_dark());
    debug!("Native window frame attached");
}

fn detach(window: &gtk4::Window) {
    FRAMES.with(|frames| {
        frames
            .borrow_mut()
            .retain(|_, frame| frame.window.upgrade().is_some_and(|other| &other != window))
    });
}

fn install_filter(display: &gdk::Display) {
    if FILTER_INSTALLED.replace(true) {
        return;
    }
    let display: *mut gdk::ffi::GdkDisplay = display.to_glib_none().0;
    unsafe { gdk_win32_display_add_filter(display.cast(), filter, std::ptr::null_mut()) };
}

fn set_dark_mode(hwnd: HWND, dark: bool) {
    // Colors the border the system draws around the window.
    let dark = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        );
    }
}

unsafe extern "C" fn filter(
    _display: *mut c_void,
    msg: *mut MSG,
    return_value: *mut c_int,
    _data: glib::ffi::gpointer,
) -> c_int {
    let msg = unsafe { &*msg };
    let Some(frame) = FRAMES.with(|frames| {
        frames
            .try_borrow()
            .ok()
            .and_then(|frames| frames.get(&(msg.hwnd as isize)).cloned())
    }) else {
        return GDK_WIN32_MESSAGE_FILTER_CONTINUE;
    };
    // Unwinding out of the filter into GTK would abort.
    match std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { handle_message(&frame, msg) })) {
        Ok(Some(value)) => {
            unsafe { *return_value = value };
            GDK_WIN32_MESSAGE_FILTER_REMOVE
        }
        Ok(None) => GDK_WIN32_MESSAGE_FILTER_CONTINUE,
        Err(_) => {
            error!("Handling window message {:#x} panicked", msg.message);
            GDK_WIN32_MESSAGE_FILTER_CONTINUE
        }
    }
}

/// Returns the result when the message is handled here, or None if GTK should handle it.
unsafe fn handle_message(frame: &Frame, msg: &MSG) -> Option<c_int> {
    let hwnd = msg.hwnd;
    match msg.message {
        WM_NCCALCSIZE => {
            // Only while the window has the frame, not in fullscreen.
            if style(hwnd) & WS_THICKFRAME == 0 {
                return None;
            }
            let rect = if msg.wParam != 0 {
                unsafe { &mut (*(msg.lParam as *mut NCCALCSIZE_PARAMS)).rgrc[0] }
            } else {
                unsafe { &mut *(msg.lParam as *mut RECT) }
            };
            if unsafe { IsZoomed(hwnd) != 0 } {
                // Maximized windows reach over the edges of the screen by their frame, so that it is not
                // visible. The content has to stay on the work area.
                if let Some(work_area) = work_area(rect) {
                    rect.left = rect.left.max(work_area.left);
                    rect.top = rect.top.max(work_area.top);
                    rect.right = rect.right.min(work_area.right);
                    rect.bottom = rect.bottom.min(work_area.bottom);
                }
            } else {
                let insets = frame_insets(hwnd);
                rect.left += insets.left;
                rect.right -= insets.right;
                rect.bottom -= insets.bottom;
            }
            Some(0)
        }
        WM_NCHITTEST => {
            if style(hwnd) & WS_THICKFRAME == 0 {
                return None;
            }
            Some(hit_test(frame, hwnd, point_from_lparam(msg.lParam)) as c_int)
        }
        WM_STYLECHANGING => {
            if msg.wParam as i32 == GWL_STYLE {
                let styles = unsafe { &mut *(msg.lParam as *mut STYLESTRUCT) };
                // GTK turns the window into a popup for fullscreen and restores the styles afterwards.
                let kept = if styles.styleNew & WS_POPUP == 0 {
                    styles.styleNew | FRAME_STYLES
                } else {
                    styles.styleNew
                };
                frame.style_kept.set(kept != styles.styleNew);
                styles.styleNew = kept;
            }
            None
        }
        WM_WINDOWPOSCHANGING => {
            let pos = unsafe { &mut *(msg.lParam as *mut WINDOWPOS) };
            if pos.flags & SWP_FRAMECHANGED != 0 && frame.style_kept.replace(false) {
                // GTK computed this geometry for the styles it wanted, but they were kept.
                pos.flags |= SWP_NOMOVE | SWP_NOSIZE;
            } else if is_from_gtk(frame, hwnd, pos) {
                let insets = frame_insets(hwnd);
                if pos.flags & SWP_NOMOVE == 0 {
                    pos.y += insets.top;
                }
                if pos.flags & SWP_NOSIZE == 0 {
                    pos.cy -= insets.top;
                }
            }
            None
        }
        WM_NCMOUSEMOVE => {
            if msg.wParam == HTMAXBUTTON as usize {
                hover_maximize_button(frame, hwnd);
            } else {
                leave_maximize_button(frame);
            }
            None
        }
        WM_NCMOUSELEAVE | WM_MOUSEMOVE => {
            leave_maximize_button(frame);
            None
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if msg.wParam == HTMAXBUTTON as usize => {
            // The system would track the press itself and draw its own button.
            hover_maximize_button(frame, hwnd);
            frame.maximize_button_pressed.set(true);
            set_maximize_button_state(frame, gtk4::StateFlags::ACTIVE, true);
            Some(0)
        }
        WM_NCLBUTTONUP if msg.wParam == HTMAXBUTTON as usize => {
            set_maximize_button_state(frame, gtk4::StateFlags::ACTIVE, false);
            if frame.maximize_button_pressed.replace(false)
                && let Some(window) = frame.window.upgrade()
            {
                // Not while the system is still handling this message.
                glib::idle_add_local_once(move || {
                    if window.is_maximized() {
                        window.unmaximize();
                    } else {
                        window.maximize();
                    }
                });
            }
            Some(0)
        }
        WM_ENTERSIZEMOVE => {
            frame.in_size_move.set(true);
            None
        }
        WM_EXITSIZEMOVE => {
            frame.in_size_move.set(false);
            None
        }
        _ => None,
    }
}

fn hover_maximize_button(frame: &Frame, hwnd: HWND) {
    if frame.maximize_button_hovered.replace(true) {
        return;
    }
    set_maximize_button_state(frame, gtk4::StateFlags::PRELIGHT, true);
    // To get a message when the pointer leaves the window from the button.
    let mut track = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe { TrackMouseEvent(&mut track) };
}

fn leave_maximize_button(frame: &Frame) {
    if !frame.maximize_button_hovered.replace(false) {
        return;
    }
    frame.maximize_button_pressed.set(false);
    set_maximize_button_state(
        frame,
        gtk4::StateFlags::PRELIGHT | gtk4::StateFlags::ACTIVE,
        false,
    );
}

fn set_maximize_button_state(frame: &Frame, state: gtk4::StateFlags, set: bool) {
    let Some(button) = frame.maximize_button.borrow().clone() else {
        return;
    };
    if set {
        button.set_state_flags(state, false);
    } else {
        button.unset_state_flags(state);
    }
}

fn style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 }
}

/// The work area of the monitor the window rectangle is on, without the taskbar.
fn work_area(rect: &RECT) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let monitor = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
        (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcWork)
    }
}

/// The frame of the window as GTK computes it, with the title bar on top.
fn frame_insets(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectEx(&mut rect, style(hwnd), 0, exstyle);
    }
    RECT {
        left: -rect.left,
        top: -rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// Whether GTK changes the geometry, which it computes with the title bar.
///
/// GTK moves and resizes its windows without activating them or changing their order. The system
/// changes the geometry for snapping, while the user drags the window or its borders, and when
/// minimizing, maximizing, restoring or leaving fullscreen.
fn is_from_gtk(frame: &Frame, hwnd: HWND, pos: &WINDOWPOS) -> bool {
    pos.flags & (SWP_NOMOVE | SWP_NOSIZE) != SWP_NOMOVE | SWP_NOSIZE
        && pos.flags & (SWP_NOACTIVATE | SWP_NOZORDER) == SWP_NOACTIVATE | SWP_NOZORDER
        && pos.flags & (SWP_STATECHANGED | SWP_FRAMECHANGED) == 0
        && !frame.in_size_move.get()
        && !frame.applying.get()
        && style(hwnd) & WS_THICKFRAME != 0
        && unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 }
        && unsafe { InSendMessageEx(std::ptr::null()) }
            & (ISMEX_SEND | ISMEX_NOTIFY | ISMEX_CALLBACK)
            == 0
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam & 0xffff) as i16 as i32,
        y: ((lparam >> 16) & 0xffff) as i16 as i32,
    }
}

fn hit_test(frame: &Frame, hwnd: HWND, point: POINT) -> u32 {
    let mut window_rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut window_rect) };

    if unsafe { IsZoomed(hwnd) == 0 } {
        let insets = frame_insets(hwnd);
        // There is no frame on top, the resize border there lies in the header bar and is thinner so
        // that it doesn't get in the way of the buttons.
        let top_border = insets.bottom / 2 + 2;
        let corner = insets.bottom * 2;

        let left = point.x < window_rect.left + insets.left;
        let right = point.x >= window_rect.right - insets.right;
        let top = point.y < window_rect.top + top_border;
        let bottom = point.y >= window_rect.bottom - insets.bottom;
        let near_left = point.x < window_rect.left + insets.left + corner;
        let near_right = point.x >= window_rect.right - insets.right - corner;
        let near_top = point.y < window_rect.top + top_border + corner;
        let near_bottom = point.y >= window_rect.bottom - insets.bottom - corner;

        if (top && near_left) || (left && near_top) {
            return HTTOPLEFT;
        } else if (top && near_right) || (right && near_top) {
            return HTTOPRIGHT;
        } else if (bottom && near_left) || (left && near_bottom) {
            return HTBOTTOMLEFT;
        } else if (bottom && near_right) || (right && near_bottom) {
            return HTBOTTOMRIGHT;
        } else if left {
            return HTLEFT;
        } else if right {
            return HTRIGHT;
        } else if top {
            return HTTOP;
        } else if bottom {
            return HTBOTTOM;
        }
    }

    let mut client_point = point;
    unsafe { ScreenToClient(hwnd, &mut client_point) };
    let area = frame
        .window
        .upgrade()
        .map_or(Area::Content, |window| content_area(&window, client_point));
    match area {
        Area::Content => HTCLIENT,
        Area::TitleBar => HTCAPTION,
        Area::MaximizeButton(button) => {
            frame.maximize_button.replace(Some(button));
            HTMAXBUTTON
        }
    }
}

/// What the point in window pixels is on: an empty part of a header bar is the title bar.
fn content_area(window: &gtk4::Window, point: POINT) -> Area {
    let Some(surface) = window.surface() else {
        return Area::Content;
    };
    let scale = surface.scale();
    let (offset_x, offset_y) = window.surface_transform();
    let Some(mut widget) = window.pick(
        point.x as f64 / scale - offset_x,
        point.y as f64 / scale - offset_y,
        gtk4::PickFlags::DEFAULT,
    ) else {
        return Area::Content;
    };
    loop {
        if widget.is::<gtk4::WindowHandle>() {
            return Area::TitleBar;
        }
        if widget.is::<gtk4::Button>()
            && widget.has_css_class("maximize")
            && widget
                .ancestor(gtk4::WindowControls::static_type())
                .is_some()
        {
            return Area::MaximizeButton(widget);
        }
        if takes_input(&widget) {
            return Area::Content;
        }
        let Some(parent) = widget.parent() else {
            return Area::Content;
        };
        widget = parent;
    }
}

fn takes_input(widget: &gtk4::Widget) -> bool {
    widget.is_focusable()
        || widget.is::<gtk4::Button>()
        || widget.is::<gtk4::Editable>()
        || widget.is::<gtk4::Range>()
}
