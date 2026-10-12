#![windows_subsystem = "windows"]

mod cursor;
mod render;
mod settings;
mod ui;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const WM_TRAY: u32 = WM_APP + 1;
const HK_TOGGLE: i32 = 1;
const HK_IN: i32 = 2;
const HK_OUT: i32 = 3;
const HK_SETTINGS: i32 = 4;
const HK_QUIT: i32 = 5;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

static REBUILD: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--restore-cursors") {
            cursor::restore_all();
            return;
        }
        let dump = args.iter().position(|a| a == "--dump").and_then(|i| args.get(i + 1)).cloned();

        // One instance only.
        let _mutex = CreateMutexW(None, true, w!("Local\\Farlens.SingleInstance")).ok();
        if dump.is_none() && GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        std::panic::set_hook(Box::new(|info| {
            cursor::restore_all();
            settings::log(&format!("panic: {info}"));
        }));

        settings::load();
        if dump.is_some() {
            settings::update(|s| s.active = true);
        }
        render::register_overlay_class();
        ui::register();

        let hi: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let cls = w!("FarlensMain");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(main_proc), hInstance: hi, lpszClassName: cls, ..Default::default() });
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), cls, w!("Farlens"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hi), None).unwrap_or_default();
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")) as isize, Ordering::Relaxed);

        if dump.is_none() {
            let _ = RegisterHotKey(Some(hwnd), HK_TOGGLE, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'Z' as u32);
            let _ = RegisterHotKey(Some(hwnd), HK_IN, MOD_CONTROL | MOD_ALT, VK_OEM_PLUS.0 as u32);
            let _ = RegisterHotKey(Some(hwnd), HK_SETTINGS, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'S' as u32);
            let _ = RegisterHotKey(Some(hwnd), HK_QUIT, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'Q' as u32);
            let _ = RegisterHotKey(Some(hwnd), HK_OUT, MOD_CONTROL | MOD_ALT, VK_OEM_MINUS.0 as u32);
            add_tray(hwnd);
            ui::create();
            if args.iter().any(|a| a == "--show-ui") {
                ui::show();
            }
        }
        // Keeps frames coming while Windows is inside a modal loop (e.g. dragging the settings window).
        MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0).ok();
        run(dump);

        // Tear the renderer down here, on purpose: leaving it to thread-local destructors at process
        // exit runs D3D calls under the loader lock and can hang, and would skip restoring cursors.
        STATE.with(|s| drop(s.borrow_mut().take()));
        cursor::restore_all();

        if let Some(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
        remove_tray(hwnd);
        settings::save_if_due();
    }
}

struct State {
    renderer: Option<render::Renderer>,
    cursors: cursor::SysCursors,
    last: Instant,
    next_hover: Instant,
    next_top: Instant,
    retry_at: Instant,
    close_start: Instant,
    hover: bool,
    frames: u32,
    dump: Option<String>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TIMER_ON: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);

/// The message pump. Frames are drawn by `tick`, which also runs from a timer so the
/// view keeps moving while Windows is in a modal loop (dragging a window, etc.).
fn run(dump: Option<String>) {
    let now = Instant::now();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            renderer: None,
            cursors: cursor::SysCursors::default(),
            last: now,
            next_hover: now,
            next_top: now,
            retry_at: now,
            close_start: now,
            hover: false,
            frames: 0,
            dump,
        })
    });

    loop {
        unsafe {
            let mut msg = MSG::default();
            // Sleep until something happens unless there is a frame to draw.
            let idle = STATE.with(|s| s.borrow().as_ref().map_or(true, |s| s.renderer.is_none())) && !settings::get().active;
            if idle {
                if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        tick();
        if QUIT.load(Ordering::Relaxed) {
            return;
        }
    }
}

fn tick() {
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(st) = guard.as_mut() else { return };
        tick_inner(st);
    });
}

/// Run the frame timer only while there is something to draw, so an idle Farlens sleeps.
fn set_timer(on: bool) {
    if TIMER_ON.swap(on, Ordering::Relaxed) == on {
        return;
    }
    let h = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    unsafe {
        if on {
            SetTimer(Some(h), 1, 8, None);
        } else {
            let _ = KillTimer(Some(h), 1);
        }
    }
}

fn tick_inner(st: &mut State) {
    settings::save_if_due();
    let s = settings::get();
    let dumping = st.dump.is_some();
    set_timer(s.active || st.renderer.is_some());

    if s.active && st.renderer.is_none() && Instant::now() >= st.retry_at {
        match render::Renderer::new() {
            Ok(r) => {
                st.renderer = Some(r);
                st.last = Instant::now();
            }
            Err(e) => {
                settings::log(&format!("renderer init failed: {e}"));
                st.retry_at = Instant::now() + Duration::from_secs(2);
            }
        }
    }
    if REBUILD.swap(false, Ordering::Relaxed) {
        st.renderer = None;
    }
    if let Some(r) = st.renderer.as_mut() {
        if !s.active && !r.closing {
            r.closing = true;
            st.close_start = Instant::now();
        } else if s.active && r.closing {
            r.closing = false;
        }
        if r.closing && (r.done || st.close_start.elapsed() > Duration::from_millis(1500)) {
            st.renderer = None;
        }
    }
    if st.renderer.is_some() && s.hide_cursor && !dumping {
        st.cursors.hide();
    } else {
        st.cursors.restore();
    }

    let Some(r) = st.renderer.as_mut() else {
        if s.active {
            std::thread::sleep(Duration::from_millis(50));
        }
        return;
    };
    let now = Instant::now();
    let dt = now.duration_since(st.last).as_secs_f32().min(0.1);
    st.last = now;

    if now >= st.next_hover {
        st.next_hover = now + Duration::from_millis(60);
        st.hover = s.hover_on && over_desktop(r.hwnd, st.hover);
    }
    if now >= st.next_top {
        st.next_top = now + Duration::from_secs(1);
        r.keep_on_top();
    }
    let zoom = s.zoom * if st.hover { s.hover_factor } else { 1.0 };

    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    st.frames += 1;
    let dump_now = match &st.dump {
        Some(p) if st.frames == 40 => Some(p.as_str()),
        _ => None,
    };
    if let Err(e) = r.frame(&s, zoom, (pt.x, pt.y), dt, &st.cursors, dump_now) {
        settings::log(&format!("frame failed, rebuilding: {e}"));
        st.renderer = None;
        st.retry_at = Instant::now() + Duration::from_millis(500);
    }
    if dumping && st.frames >= 41 {
        QUIT.store(true, Ordering::Relaxed);
        // The timer keeps the pump busy, so also post WM_QUIT to make it return.
        unsafe { PostQuitMessage(0) };
    }
}
/// Is the pointer over the bare desktop (wallpaper / icons) rather than a window?
fn over_desktop(overlay: HWND, previous: bool) -> bool {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return previous;
        }
        let h = WindowFromPoint(pt);
        if h.is_invalid() || h == overlay {
            return previous;
        }
        let root = GetAncestor(h, GA_ROOT);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(if root.is_invalid() { h } else { root }, &mut buf) as usize;
        let name = String::from_utf16_lossy(&buf[..n]);
        name == "Progman" || name == "WorkerW"
    }
}

unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && w.0 as u32 == WM_MOUSEWHEEL {
            let down = |vk: VIRTUAL_KEY| GetAsyncKeyState(vk.0 as i32) < 0;
            if down(VK_CONTROL) && down(VK_MENU) {
                let info = &*(l.0 as *const MSLLHOOKSTRUCT);
                let delta = (info.mouseData >> 16) as i16 as f32 / 120.0;
                zoom_by(1.15f32.powf(delta));
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, w, l)
    }
}

fn zoom_by(f: f32) {
    settings::update(|s| s.zoom *= f);
    ui::sync();
}

unsafe extern "system" fn main_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_TIMER => {
                tick();
                LRESULT(0)
            }
            WM_HOTKEY => {
                match w.0 as i32 {
                    HK_TOGGLE => toggle(),
                    HK_IN => zoom_by(1.2),
                    HK_OUT => zoom_by(1.0 / 1.2),
                    HK_SETTINGS => ui::show(),
                    HK_QUIT => PostQuitMessage(0),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAY => {
                match (l.0 & 0xFFFF) as u32 {
                    // Any click on the tray icon: open the control panel and move the mouse to it.
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONUP => ui::show(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                // Rebuild the renderer for the new resolution.
                REBUILD.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            _ if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) as u32 => {
                add_tray(h);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

fn toggle() {
    settings::update(|s| s.active = !s.active);
    ui::sync();
}

/// The app icon (resource 1, baked in by build.rs) at the given pixel size.
pub fn app_icon(size: i32) -> HICON {
    unsafe {
        let hi = GetModuleHandleW(None).unwrap_or_default();
        match LoadImageW(Some(hi.into()), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
            Ok(h) => HICON(h.0),
            Err(_) => HICON::default(),
        }
    }
}

fn tray_icon() -> HICON {
    app_icon(unsafe { GetSystemMetrics(SM_CXSMICON) })
}

fn add_tray(h: HWND) {
    unsafe {
        let mut nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: h, uID: 1, uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP, uCallbackMessage: WM_TRAY, hIcon: tray_icon(), ..Default::default() };
        let tip: Vec<u16> = "Farlens".encode_utf16().collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray(h: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: h, uID: 1, ..Default::default() };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_READ, &mut key).is_err() {
            return false;
        }
        let r = RegQueryValueExW(key, w!("Farlens"), None, None, None, None);
        let _ = RegCloseKey(key);
        r.is_ok()
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_WRITE, &mut key).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
                let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
                let _ = RegSetValueExW(key, w!("Farlens"), None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, w!("Farlens"));
        }
        let _ = RegCloseKey(key);
    }
}
