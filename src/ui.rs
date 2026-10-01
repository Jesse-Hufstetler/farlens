//! The settings window: plain Win32 trackbars, checkboxes and radio buttons,
//! driven by a table so adding an option is one line.

use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
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
}

struct Ctl {
    hwnd: HWND,
    id: i32,
    kind: Kind,
}

struct Ui {
    hwnd: HWND,
    ctls: Vec<Ctl>,
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

fn x1(v: f32) -> String { format!("{v:.1}×") }
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
        let text = self.child(w!("STATIC"), label, 0, 12, self.y + 4, 190, 20, -1);
        let tb = self.child(w!("msctls_trackbar32"), "", (TBS_HORZ | TBS_NOTICKS) as u32, 200, self.y, 220, 28, id);
        unsafe {
            let n = if log { LOG_STEPS as isize } else { ((max - min) / step).round() as isize };
            SendMessageW(tb, TBM_SETRANGE, Some(WPARAM(1)), Some(LPARAM(n << 16)));
        }
        self.ctls.push(Ctl { hwnd: tb, id, kind: Kind::Slider { min, max, step, log, label, get, set, fmt, text } });
        self.y += 30;
    }

    fn check(&mut self, label: &str, x: i32, w: i32, get: fn(&Settings) -> bool, set: fn(&mut Settings, bool)) {
        let id = self.id();
        let hw = self.child(w!("BUTTON"), label, BS_AUTOCHECKBOX as u32, x, self.y, w, 22, id);
        self.ctls.push(Ctl { hwnd: hw, id, kind: Kind::Check { get, set } });
    }

    fn radio(&mut self, label: &str, x: i32, w: i32, first: bool, on: fn(&Settings) -> bool, pick: fn(&mut Settings)) {
        let id = self.id();
        let style = BS_AUTORADIOBUTTON as u32 | if first { WS_GROUP.0 } else { 0 };
        let hw = self.child(w!("BUTTON"), label, style, x, self.y, w, 22, id);
        self.ctls.push(Ctl { hwnd: hw, id, kind: Kind::Radio { on, pick } });
    }
}

pub fn create() {
    unsafe {
        let hi: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Farlens"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
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
        let font = CreateFontW(-((9.0 * dpi as f32) / 72.0).round() as i32, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI"));

        let mut l = Layout { y: 8, s, font, parent: hwnd, hi, next_id: 1000, ctls: Vec::new() };

        l.check("Magnifier on   (Ctrl+Alt+Z)", 12, 300, |s| s.active, |s, v| s.active = v);
        l.y += 26;

        l.heading("Camera");
        l.slider("Zoom", 0.25, 16.0, 0.05, |s| s.zoom, |s, v| s.zoom = v, x2_zoom);
        l.log_slider("Chase lag", 0.005, 3.0, |s| s.lag, |s, v| s.lag = v, secs);
        l.slider("Max tilt", 5.0, 80.0, 1.0, |s| s.max_tilt, |s, v| s.max_tilt = v, deg);
        l.check("Chase the mouse (let it drift off-center)", 12, 400, |s| s.aim_chase, |s, v| s.aim_chase = v);
        l.y += 26;
        l.log_slider("Aim lag", 0.005, 3.0, |s| s.aim_lag, |s, v| s.aim_lag = v, secs);
        l.check("Hide the real mouse cursor", 12, 300, |s| s.hide_cursor, |s, v| s.hide_cursor = v);
        l.y += 26;
        l.check("Show the normal desktop beyond the plane's edge", 12, 400, |s| s.bg_desktop, |s, v| s.bg_desktop = v);
        l.y += 26;

        l.heading("Warp");
        l.check("Warp on", 12, 120, |s| s.warp_on, |s, v| s.warp_on = v);
        l.radio("Circular fisheye", 140, 140, true, |s| s.warp_shape == Shape::Circle, |s| s.warp_shape = Shape::Circle);
        l.radio("Rectangular", 290, 120, false, |s| s.warp_shape == Shape::Rect, |s| s.warp_shape = Shape::Rect);
        l.y += 26;
        l.slider("Clear radius", 0.0, 4.0, 0.01, |s| s.warp_radius, |s, v| s.warp_radius = v, x2);
        l.slider("Warp amount", 0.0, 30.0, 0.1, |s| s.warp_amount, |s, v| s.warp_amount = v, x2);
        l.check("Top", 12, 80, |s| s.warp_top, |s, v| s.warp_top = v);
        l.check("Bottom", 100, 80, |s| s.warp_bottom, |s, v| s.warp_bottom = v);
        l.check("Left", 190, 80, |s| s.warp_left, |s, v| s.warp_left = v);
        l.check("Right", 280, 80, |s| s.warp_right, |s, v| s.warp_right = v);
        l.y += 26;

        l.heading("Context");
        l.check("Zoom out while the mouse is over the bare desktop", 12, 400, |s| s.hover_on, |s, v| s.hover_on = v);
        l.y += 26;
        l.slider("Desktop zoom", 0.1, 0.95, 0.01, |s| s.hover_factor, |s, v| s.hover_factor = v, x2_mul);

        l.y += 8;
        l.check("Start with Windows", 12, 300, |_| crate::autostart_enabled(), |_, v| crate::set_autostart(v));
        l.y += 30;
        l.child(w!("STATIC"), "Ctrl+Alt+wheel (or Ctrl+Alt + / -) changes zoom.", 0, 12, l.y, 400, 18, -1);
        l.y += 26;

        let (cw, ch) = ((432.0 * s) as i32, (l.y as f32 * s) as i32);
        let _ = SetWindowPos(hwnd, None, 0, 0, cw, ch, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        // Grow by the frame so the client area ends up exactly cw x ch.
        let (mut wr, mut cr) = (RECT::default(), RECT::default());
        let _ = GetWindowRect(hwnd, &mut wr);
        let _ = GetClientRect(hwnd, &mut cr);
        let (fw, fh) = ((wr.right - wr.left) - cr.right, (wr.bottom - wr.top) - cr.bottom);
        let _ = SetWindowPos(hwnd, None, 0, 0, cw + fw, ch + fh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        let ctls = l.ctls;
        UI.with(|u| *u.borrow_mut() = Some(Ui { hwnd, ctls }));
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
                    Kind::Radio { on, .. } => {
                        SendMessageW(c.hwnd, BM_SETCHECK, Some(WPARAM(on(&s) as usize)), Some(LPARAM(0)));
                    }
                }
            }
        }
    });
}

pub fn show() {
    UI.with(|u| {
        if let Some(u) = u.borrow().as_ref() {
            unsafe {
                let _ = ShowWindow(u.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(u.hwnd);
            }
        }
    });
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
            WM_COMMAND => {
                let id = (w.0 & 0xFFFF) as i32;
                let code = ((w.0 >> 16) & 0xFFFF) as u32;
                if code == BN_CLICKED {
                    enum Act { Check(fn(&mut Settings, bool), bool), Radio(fn(&mut Settings)) }
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
                                    _ => None,
                                };
                            }
                        }
                        None
                    });
                    match act {
                        Some(Act::Check(set, on)) => settings::update(|s| set(s, on)),
                        Some(Act::Radio(pick)) => settings::update(|s| pick(s)),
                        None => {}
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = ShowWindow(h, SW_HIDE);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}
