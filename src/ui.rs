//! The settings window: plain Win32 trackbars, checkboxes and radio buttons,
//! driven by a table so adding an option is one line.

use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::settings::{self, Settings, Shape};

type Get = fn(&Settings) -> f32;
type Set = fn(&mut Settings, f32);
type Fmt = fn(f32) -> String;

enum Kind {
    Slider { min: f32, max: f32, step: f32, log: bool, label: &'static str, get: Get, set: Set, fmt: Fmt, text: HWND },
    Check { get: fn(&Settings) -> bool, set: fn(&mut Settings, bool) },
    Radio { on: fn(&Settings) -> bool, pick: fn(&mut Settings) },
    Action { run: fn() },
}

struct Ctl {
    hwnd: HWND,
    id: i32,
    kind: Kind,
}

struct Ui {
    hwnd: HWND,
    ctls: Vec<Ctl>,
    /// Height of all the controls in 96-dpi units (the scroll range).
    content_h: i32,
    /// Smallest outer window size, in pixels.
    min_size: (i32, i32),
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

const TBM_GETPOS_: u32 = 0x400;
const CLASS: PCWSTR = w!("FarlensSettings");

pub fn register() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        let hi = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: hi.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

fn px(v: f32) -> String { format!("{v:.0} px") }
fn x2(v: f32) -> String { format!("{v:.2}") }
fn secs(v: f32) -> String { if v <= 0.001 { "locked on".into() } else { format!("{:.0} ms", v * 1000.0) } }
fn deg(v: f32) -> String { format!("{v:.0}°") }

struct Layout {
    y: i32,
    s: f32,
    font: HFONT,
    parent: HWND,
    hi: HINSTANCE,
    next_id: i32,
    ctls: Vec<Ctl>,
    /// The tooltip control, and the controls the next `tip` call applies to.
    tip_hwnd: HWND,
    last: Vec<HWND>,
}

impl Layout {
    fn px(&self, v: i32) -> i32 { (v as f32 * self.s).round() as i32 }

    fn child(&self, class: PCWSTR, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
        unsafe {
            let t = HSTRING::from(text);
            let hw = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &t,
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                self.px(x),
                self.px(y),
                self.px(w),
                self.px(h),
                Some(self.parent),
                Some(HMENU(id as usize as *mut _)),
                Some(self.hi),
                None,
            )
            .unwrap_or_default();
            SendMessageW(hw, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
            hw
        }
    }

    fn id(&mut self) -> i32 { self.next_id += 1; self.next_id }

    /// Give the control(s) just added (a slider's label and track, or a single control) a tooltip.
    fn tip(&mut self, text: &str) {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let ptr = Box::leak(wide.into_boxed_slice()).as_mut_ptr(); // lives as long as the window
        for &hw in &self.last {
            let ti = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                hwnd: self.parent,
                uId: hw.0 as usize,
                lpszText: PWSTR(ptr),
                ..Default::default()
            };
            unsafe {
                SendMessageW(self.tip_hwnd, TTM_ADDTOOLW, None, Some(LPARAM(&ti as *const _ as isize)));
            }
        }
    }

    fn heading(&mut self, text: &str) {
        self.y += 6;
        self.child(w!("STATIC"), text, 0, 12, self.y, 400, 18, -1);
        self.y += 22;
    }

    fn slider(&mut self, label: &'static str, min: f32, max: f32, step: f32, get: Get, set: Set, fmt: Fmt) {
        self.slider_ex(label, min, max, step, false, get, set, fmt);
    }

    /// `log`: positions are spaced geometrically from `min` (the smallest nonzero value) to `max`; the far-left stop is 0.
    fn log_slider(&mut self, label: &'static str, min: f32, max: f32, get: Get, set: Set, fmt: Fmt) {
        self.slider_ex(label, min, max, 0.0, true, get, set, fmt);
    }

    fn slider_ex(&mut self, label: &'static str, min: f32, max: f32, step: f32, log: bool, get: Get, set: Set, fmt: Fmt) {
        let id = self.id();
        let text = self.child(w!("STATIC"), label, 0x100 /* SS_NOTIFY: so the label gets mouse hover */, 12, self.y + 4, 190, 20, -1);
        let tb = self.child(w!("msctls_trackbar32"), "", (TBS_HORZ | TBS_NOTICKS) as u32, 200, self.y, 220, 28, id);
        unsafe {
            let n = if log { LOG_STEPS as isize } else { ((max - min) / step).round() as isize };
            SendMessageW(tb, TBM_SETRANGE, Some(WPARAM(1)), Some(LPARAM(n << 16)));
        }
        self.ctls.push(Ctl { hwnd: tb, id, kind: Kind::Slider { min, max, step, log, label, get, set, fmt, text } });
        self.last = vec![text, tb];
        self.y += 30;
    }

    fn check(&mut self, label: &str, x: i32, w: i32, get: fn(&Settings) -> bool, set: fn(&mut Settings, bool)) {
        let id = self.id();
        let hw = self.child(w!("BUTTON"), label, BS_AUTOCHECKBOX as u32, x, self.y, w, 22, id);
        self.ctls.push(Ctl { hwnd: hw, id, kind: Kind::Check { get, set } });
        self.last = vec![hw];
    }

    fn button(&mut self, label: &str, x: i32, w: i32, run: fn()) {
        let id = self.id();
        let hw = self.child(w!("BUTTON"), label, BS_PUSHBUTTON as u32, x, self.y, w, 28, id);
        self.ctls.push(Ctl { hwnd: hw, id, kind: Kind::Action { run } });
        self.last = vec![hw];
    }

    fn radio(&mut self, label: &str, x: i32, w: i32, first: bool, on: fn(&Settings) -> bool, pick: fn(&mut Settings)) {
        let id = self.id();
        let style = BS_AUTORADIOBUTTON as u32 | if first { WS_GROUP.0 } else { 0 };
        let hw = self.child(w!("BUTTON"), label, style, x, self.y, w, 22, id);
        self.ctls.push(Ctl { hwnd: hw, id, kind: Kind::Radio { on, pick } });
        self.last = vec![hw];
    }
}

pub fn create() {
    unsafe {
        let hi: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(concat!("Farlens ", env!("CARGO_PKG_VERSION"))),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME | WS_VSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(hi),
            None,
        )
        .unwrap_or_default();
        let dpi = GetDpiForWindow(hwnd).max(96);
        let s = dpi as f32 / 96.0;
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let icon = crate::app_icon(GetSystemMetricsForDpi(metric, dpi));
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(which as usize)), Some(LPARAM(icon.0 as isize)));
        }
        let font = CreateFontW(-((9.0 * dpi as f32) / 72.0).round() as i32, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI"));

        let tip_hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            None,
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(hwnd),
            None,
            Some(hi),
            None,
        )
        .unwrap_or_default();
        SendMessageW(tip_hwnd, TTM_SETMAXTIPWIDTH, None, Some(LPARAM((340.0 * s) as isize)));
        SendMessageW(tip_hwnd, TTM_SETDELAYTIME, Some(WPARAM(3)), Some(LPARAM(400))); // initial delay
        SendMessageW(tip_hwnd, TTM_SETDELAYTIME, Some(WPARAM(2)), Some(LPARAM(30000))); // stays up

        let mut l = Layout { y: 8, s, font, parent: hwnd, hi, next_id: 1000, ctls: Vec::new(), tip_hwnd, last: Vec::new() };

        l.check("Magnifier on   (Ctrl+Alt+Z)", 12, 300, |s| s.active, |s, v| s.active = v);
        l.tip("Turns the magnifier on or off. It eases in and out.");
        l.y += 26;

        l.heading("Camera");
        l.slider("Zoom", 0.25, 16.0, 0.05, |s| s.zoom, |s, v| s.zoom = v, x2_zoom);
        l.tip("How much the screen is magnified. Also Ctrl+Alt+mouse wheel, or Ctrl+Alt and + / -.");
        l.log_slider("Mouse smoothing", 0.005, 1.0, |s| s.smooth, |s, v| s.smooth = v, secs);
        l.tip("Smooths out your raw mouse movement before the camera sees it. Higher hides jitter but makes the view react later. Far left is no smoothing.");
        l.log_slider("Chase lag", 0.005, 3.0, |s| s.lag, |s, v| s.lag = v, secs);
        l.tip("How long the camera takes to catch up with the mouse. While it catches up, the view leans toward where you are heading. Far left locks the camera onto the mouse.");
        l.slider("Lag skew", 0.0, 8.0, 0.1, |s| s.lag_skew, |s, v| s.lag_skew = v, x2);
        l.tip("Makes the camera lag more up/down than side to side, since there is less screen to look ahead into vertically. Up/down lag divided by side-to-side lag is the screen's aspect ratio raised to this power: 0 is the same both ways, 2 is about 3:1 on a 16:9 screen. The Chase lag slider stays the middle of the two.");
        l.slider("Max tilt", 5.0, 80.0, 1.0, |s| s.max_tilt, |s, v| s.max_tilt = v, deg);
        l.tip("The furthest the camera may lean away from looking straight at the screen, in degrees. Lower keeps the view flatter, higher allows a stronger 3D tilt.");
        l.check("Chase the mouse (flat view, mouse may drift off-center)", 12, 400, |s| s.aim_chase, |s, v| s.aim_chase = v);
        l.tip("Flat view: the camera follows the mouse head-on instead of tilting toward it, so the mouse can drift away from the center of the view.");
        l.y += 26;
        l.check("Hide the real mouse cursor", 12, 300, |s| s.hide_cursor, |s, v| s.hide_cursor = v);
        l.tip("Hides the system mouse cursor and draws one on the magnified screen instead, so you only see a single cursor.");
        l.y += 26;
        l.check("Show the normal desktop beyond the plane's edge", 12, 400, |s| s.bg_desktop, |s, v| s.bg_desktop = v);
        l.tip("Beyond the edge of the magnified screen, show the ordinary unmagnified desktop instead of the plain background.");
        l.y += 26;

        l.heading("Warp");
        l.check("Warp on", 12, 120, |s| s.warp_on, |s, v| s.warp_on = v);
        l.tip("Bends the screen so the center stays at full zoom while the edges show progressively more of the surroundings.");
        l.radio("Circular fisheye", 140, 140, true, |s| s.warp_shape == Shape::Circle, |s| s.warp_shape = Shape::Circle);
        l.tip("A round fisheye lens: distance from the center sets how much it is compressed.");
        l.radio("Rectangular", 290, 120, false, |s| s.warp_shape == Shape::Rect, |s| s.warp_shape = Shape::Rect);
        l.tip("Warps each axis on its own, like a wide-angle view of a flat screen. The side checkboxes below pick which edges bend.");
        l.y += 26;
        l.slider("Clear radius", 0.0, 4.0, 0.01, |s| s.warp_radius, |s, v| s.warp_radius = v, x2);
        l.tip("How big the center area is that is left at full zoom with no warp. 0 starts a smooth bulge right at the center.");
        l.log_slider("Warp amount", 0.05, 30.0, |s| s.warp_amount, |s, v| s.warp_amount = v, x2);
        l.tip("How strongly the edges are compressed. Small values go a long way, so this slider is logarithmic. Far left is none.");
        l.check("Top", 12, 80, |s| s.warp_top, |s, v| s.warp_top = v);
        l.tip("Rectangular warp only: warp the top edge. Uncheck to leave it undistorted.");
        l.check("Bottom", 100, 80, |s| s.warp_bottom, |s, v| s.warp_bottom = v);
        l.tip("Rectangular warp only: warp the bottom edge. Uncheck to leave it undistorted.");
        l.check("Left", 190, 80, |s| s.warp_left, |s, v| s.warp_left = v);
        l.tip("Rectangular warp only: warp the left edge. Uncheck to leave it undistorted.");
        l.check("Right", 280, 80, |s| s.warp_right, |s, v| s.warp_right = v);
        l.tip("Rectangular warp only: warp the right edge. Uncheck to leave it undistorted.");
        l.y += 26;

        l.heading("Pane");
        l.slider("Shadow", 0.0, 1.0, 0.01, |s| s.shadow, |s, v| s.shadow = v, x2);
        l.tip("How dark the drop shadow under the magnified screen is. 0 turns it off.");
        l.slider("Shadow size", 2.0, 150.0, 1.0, |s| s.shadow_size, |s, v| s.shadow_size = v, px);
        l.tip("How far the shadow reaches, in screen pixels.");
        l.slider("Background", 0.0, 0.5, 0.005, |s| s.bg_level, |s, v| s.bg_level = v, x2);
        l.tip("Brightness of the empty area around the magnified screen, when the normal desktop is not shown there.");

        l.heading("Context");
        l.check("Zoom out while the mouse is over the bare desktop", 12, 400, |s| s.hover_on, |s, v| s.hover_on = v);
        l.tip("Zooms out when the mouse is over the bare desktop (wallpaper and icons) rather than a window, so you can see the whole desktop.");
        l.y += 26;
        l.slider("Desktop zoom", 0.1, 0.95, 0.01, |s| s.hover_factor, |s, v| s.hover_factor = v, x2_mul);
        l.tip("The zoom used over the bare desktop, as a percentage of your normal zoom.");

        l.y += 8;
        l.check("Start with Windows", 12, 300, |_| crate::autostart_enabled(), |_, v| crate::set_autostart(v));
        l.tip("Starts Farlens automatically when you sign in to Windows.");
        l.y += 30;
        l.heading("Hotkeys (work from anywhere)");
        l.child(w!("STATIC"), "Ctrl+Alt+Z  -  magnifier on / off\r\nCtrl+Alt+S  -  open this window (moves the mouse here)\r\nCtrl+Alt+Q  -  quit Farlens\r\nCtrl+Alt+wheel, or Ctrl+Alt + / -  -  zoom", 0, 12, l.y, 410, 80, -1);
        l.y += 84;
        l.button("Quit Farlens", 12, 140, crate::request_quit);
        l.tip("Eases back to the plain desktop and exits (Ctrl+Alt+Q).");
        l.y += 4;
        l.y += 26;

        let (cw, ch) = ((432.0 * s) as i32, (l.y as f32 * s) as i32);
        let _ = SetWindowPos(hwnd, None, 0, 0, cw, ch, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        // Grow by the frame so the client area ends up exactly cw x ch.
        let (mut wr, mut cr) = (RECT::default(), RECT::default());
        let _ = GetWindowRect(hwnd, &mut wr);
        let _ = GetClientRect(hwnd, &mut cr);
        let (fw, fh) = ((wr.right - wr.left) - cr.right, (wr.bottom - wr.top) - cr.bottom);
        let (mut win_w, mut win_h) = (cw + fw, ch + fh);
        // On a short screen, cap the height at the work area and scroll instead.
        let work = work_area(hwnd);
        let max_h = work.bottom - work.top;
        let sbw = GetSystemMetricsForDpi(SM_CXVSCROLL, dpi);
        if win_h > max_h {
            win_w += sbw;
            win_h = max_h;
        }
        let min_size = (cw + fw + sbw, (240.0 * s) as i32 + fh);
        let _ = SetWindowPos(hwnd, None, 0, 0, win_w, win_h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        let ctls = std::mem::take(&mut l.ctls);
        place(hwnd);
        fit_on_screen(hwnd);
        let content_h = l.y;
        UI.with(|u| *u.borrow_mut() = Some(Ui { hwnd, ctls, content_h, min_size }));
        on_size(hwnd);
        sync();
    }
}

fn x2_zoom(v: f32) -> String { format!("{v:.2}×") }
fn x2_mul(v: f32) -> String { format!("{:.0}% of zoom", v * 100.0) }

const LOG_STEPS: usize = 1000;

fn slider_pos(min: f32, max: f32, step: f32, log: bool, v: f32) -> isize {
    if !log {
        return ((v - min) / step).round() as isize;
    }
    if v < min * 0.5 {
        return 0;
    }
    let t = (v / min).ln() / (max / min).ln();
    (1.0 + t.clamp(0.0, 1.0) * (LOG_STEPS - 1) as f32).round() as isize
}

fn slider_value(min: f32, max: f32, step: f32, log: bool, pos: isize) -> f32 {
    if !log {
        return (min + pos as f32 * step).clamp(min, max);
    }
    if pos <= 0 {
        return 0.0;
    }
    min * (max / min).powf((pos - 1) as f32 / (LOG_STEPS - 1) as f32)
}

/// Push the current settings into the controls (after a hotkey or the tray changed them).
pub fn sync() {
    let s = settings::get();
    UI.with(|u| {
        let u = u.borrow();
        let Some(u) = u.as_ref() else { return };
        unsafe {
            for c in &u.ctls {
                match &c.kind {
                    Kind::Slider { min, max, step, log, label, get, fmt, text, .. } => {
                        let v = get(&s);
                        SendMessageW(c.hwnd, TBM_SETPOS, Some(WPARAM(1)), Some(LPARAM(slider_pos(*min, *max, *step, *log, v))));
                        let _ = SetWindowTextW(*text, &HSTRING::from(format!("{label}:  {}", fmt(v))));
                    }
                    Kind::Check { get, .. } => {
                        SendMessageW(c.hwnd, BM_SETCHECK, Some(WPARAM(get(&s) as usize)), Some(LPARAM(0)));
                    }
                    Kind::Action { .. } => {}
                    Kind::Radio { on, .. } => {
                        SendMessageW(c.hwnd, BM_SETCHECK, Some(WPARAM(on(&s) as usize)), Some(LPARAM(0)));
                    }
                }
            }
        }
    });
}

fn work_area(hwnd: HWND) -> RECT {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1024, bottom: 768 }
        }
    }
}

/// Nudge the window so it lies inside the work area of the monitor it is on.
fn fit_on_screen(hwnd: HWND) {
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return;
        }
        let w = work_area(hwnd);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let x = r.left.min(w.right - ww).max(w.left);
        let y = r.top.min(w.bottom - wh).max(w.top);
        if (x, y) != (r.left, r.top) {
            let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// After any resize: stretch the sliders to the new width and fit the scroll range to the new height.
fn on_size(h: HWND) {
    unsafe {
        let mut cr = RECT::default();
        if GetClientRect(h, &mut cr).is_err() {
            return;
        }
        let s = GetDpiForWindow(h).max(96) as f32 / 96.0;
        let slider_w = (cr.right - (212.0 * s) as i32).max((220.0 * s) as i32);
        let content = UI.with(|u| {
            let u = u.borrow();
            let u = u.as_ref()?;
            for c in &u.ctls {
                if matches!(c.kind, Kind::Slider { .. }) {
                    let mut r = RECT::default();
                    if GetWindowRect(c.hwnd, &mut r).is_ok() {
                        let _ = SetWindowPos(c.hwnd, None, 0, 0, slider_w, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
            }
            Some((u.content_h as f32 * s) as i32)
        });
        let Some(content) = content else { return };
        let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
        if GetScrollInfo(h, SB_VERT, &mut si).is_err() {
            return;
        }
        let old = si.nPos;
        si.fMask = SIF_RANGE | SIF_PAGE | SIF_POS;
        si.nMin = 0;
        si.nMax = content - 1;
        si.nPage = cr.bottom as u32;
        SetScrollInfo(h, SB_VERT, &si, true);
        // The system clamps the position when the page grows; move the controls to match.
        let mut now = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_POS, ..Default::default() };
        if GetScrollInfo(h, SB_VERT, &mut now).is_ok() && now.nPos != old {
            let _ = ScrollWindowEx(h, 0, old - now.nPos, None, None, None, None, SW_SCROLLCHILDREN | SW_INVALIDATE | SW_ERASE);
        }
        let _ = InvalidateRect(Some(h), None, true);
    }
}

/// Scroll the content so `pos` (clamped) is the top of the visible area.
fn scroll_to(h: HWND, pos: i32) {
    unsafe {
        let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
        if GetScrollInfo(h, SB_VERT, &mut si).is_err() {
            return;
        }
        let top = (si.nMax - si.nPage as i32 + 1).max(0);
        let pos = pos.clamp(0, top);
        let dy = si.nPos - pos;
        if dy == 0 {
            return;
        }
        si.fMask = SIF_POS;
        si.nPos = pos;
        SetScrollInfo(h, SB_VERT, &si, true);
        let _ = ScrollWindowEx(h, 0, dy, None, None, None, None, SW_SCROLLCHILDREN | SW_INVALIDATE | SW_ERASE);
    }
}

/// Move the window to where it was last left, unless that spot is no longer on any monitor.
fn place(hwnd: HWND) {
    let s = settings::get();
    if s.win_x == i32::MIN || s.win_y == i32::MIN {
        return;
    }
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return;
        }
        let want = RECT { left: s.win_x, top: s.win_y, right: s.win_x + (r.right - r.left), bottom: s.win_y + (r.bottom - r.top) };
        // Require the title bar area to be on a monitor, so the window can always be grabbed.
        let bar = RECT { left: want.left, top: want.top, right: want.right, bottom: want.top + 40 };
        if !MonitorFromRect(&bar, MONITOR_DEFAULTTONULL).is_invalid() {
            let _ = SetWindowPos(hwnd, None, want.left, want.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

fn remember(hwnd: HWND) {
    unsafe {
        let mut r = RECT::default();
        if IsIconic(hwnd).as_bool() || GetWindowRect(hwnd, &mut r).is_err() {
            return;
        }
        settings::update(|s| {
            s.win_x = r.left;
            s.win_y = r.top;
        });
    }
}

/// Bring the window up and put the mouse on its title bar, so the magnified view jumps
/// to it (it sits at its real screen position, wherever the camera currently looks).
pub fn show() {
    let hwnd = UI.with(|u| u.borrow().as_ref().map(|u| u.hwnd));
    let Some(hwnd) = hwnd else { return };
    unsafe {
        place(hwnd);
        fit_on_screen(hwnd);
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_ok() {
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let _ = SetCursorPos((r.left + r.right) / 2, r.top + 16 * dpi / 96);
        }
    }
}
unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_HSCROLL => {
                let tb = HWND(l.0 as *mut _);
                let pos = SendMessageW(tb, TBM_GETPOS_, None, None).0;
                let hit = UI.with(|u| {
                    let u = u.borrow();
                    let u = u.as_ref()?;
                    for c in &u.ctls {
                        if c.hwnd == tb {
                            if let Kind::Slider { min, max, step, log, label, set, fmt, text, .. } = &c.kind {
                                let v = slider_value(*min, *max, *step, *log, pos);
                                return Some((*set, v, format!("{label}:  {}", fmt(v)), *text));
                            }
                        }
                    }
                    None
                });
                if let Some((set, v, label, text)) = hit {
                    let _ = SetWindowTextW(text, &HSTRING::from(label));
                    settings::update(|s| set(s, v));
                }
                LRESULT(0)
            }
            WM_SIZE => {
                on_size(h);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let min = UI.with(|u| u.borrow().as_ref().map(|u| u.min_size));
                if let Some((mw, mh)) = min {
                    let mmi = &mut *(l.0 as *mut MINMAXINFO);
                    mmi.ptMinTrackSize.x = mw;
                    mmi.ptMinTrackSize.y = mh;
                }
                LRESULT(0)
            }
            WM_VSCROLL => {
                let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
                if GetScrollInfo(h, SB_VERT, &mut si).is_ok() {
                    let line = 30 * GetDpiForWindow(h).max(96) as i32 / 96;
                    let page = si.nPage as i32;
                    let pos = match SCROLLBAR_COMMAND((w.0 & 0xFFFF) as i32) {
                        SB_LINEUP => si.nPos - line,
                        SB_LINEDOWN => si.nPos + line,
                        SB_PAGEUP => si.nPos - page,
                        SB_PAGEDOWN => si.nPos + page,
                        SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos,
                        SB_TOP => 0,
                        SB_BOTTOM => i32::MAX / 2,
                        _ => si.nPos,
                    };
                    scroll_to(h, pos);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
                if GetScrollInfo(h, SB_VERT, &mut si).is_ok() {
                    let notches = ((w.0 >> 16) as u16 as i16 as i32) / 120;
                    let line = 30 * GetDpiForWindow(h).max(96) as i32 / 96;
                    scroll_to(h, si.nPos - notches * line * 3);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (w.0 & 0xFFFF) as i32;
                let code = ((w.0 >> 16) & 0xFFFF) as u32;
                if code == BN_CLICKED {
                    enum Act { Check(fn(&mut Settings, bool), bool), Radio(fn(&mut Settings)), Run(fn()) }
                    let act = UI.with(|u| {
                        let u = u.borrow();
                        let u = u.as_ref()?;
                        for c in &u.ctls {
                            if c.id == id {
                                return match &c.kind {
                                    Kind::Check { set, .. } => {
                                        let on = SendMessageW(c.hwnd, BM_GETCHECK, None, None).0 == 1;
                                        Some(Act::Check(*set, on))
                                    }
                                    Kind::Radio { pick, .. } => Some(Act::Radio(*pick)),
                                    Kind::Action { run } => Some(Act::Run(*run)),
                                    _ => None,
                                };
                            }
                        }
                        None
                    });
                    match act {
                        Some(Act::Check(set, on)) => settings::update(|s| set(s, on)),
                        Some(Act::Radio(pick)) => settings::update(|s| pick(s)),
                        Some(Act::Run(run)) => run(),
                        None => {}
                    }
                }
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                remember(h);
                LRESULT(0)
            }
            WM_CLOSE => {
                remember(h);
                let _ = ShowWindow(h, SW_HIDE);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}
