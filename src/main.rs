#![windows_subsystem = "windows"]

mod cursor;
mod render;
mod settings;
mod ui;

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
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
const CMD_TOGGLE: usize = 1;
const CMD_SETTINGS: usize = 2;
const CMD_EXIT: usize = 3;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

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
            let _ = RegisterHotKey(Some(hwnd), HK_OUT, MOD_CONTROL | MOD_ALT, VK_OEM_MINUS.0 as u32);
            add_tray(hwnd);
            ui::create();
            if args.iter().any(|a| a == "--show-ui") {
                ui::show();
            }
        }
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0).ok();

        run(dump);

        if let Some(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
        remove_tray(hwnd);
        settings::save_if_due();
    }
}

/// The message pump and the frame loop in one.
fn run(dump: Option<String>) {
    let mut renderer: Option<render::Renderer> = None;
    let mut cursors = cursor::SysCursors::default();
    let mut last = Instant::now();
    let mut next_hover = Instant::now();
    let mut next_top = Instant::now();
    let mut retry_at = Instant::now();
    let mut hover = false;
    let mut frames = 0u32;

    loop {
        unsafe {
            let mut msg = MSG::default();
            let s = settings::get();
            // Sleep until something happens unless there is a frame to draw.
            if renderer.is_none() && !s.active {
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
        settings::save_if_due();
        let s = settings::get();

        if s.active && renderer.is_none() && Instant::now() >= retry_at {
            match render::Renderer::new() {
                Ok(r) => {
                    renderer = Some(r);
                    last = Instant::now();
                }
                Err(e) => {
                    settings::log(&format!("renderer init failed: {e}"));
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }
        if !s.active && renderer.is_some() {
            renderer = None;
        }
        if renderer.is_some() && s.hide_cursor && dump.is_none() {
            cursors.hide();
        } else {
            cursors.restore();
        }

        let Some(r) = renderer.as_mut() else {
            if s.active {
                std::thread::sleep(Duration::from_millis(50));
            }
            continue;
        };
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;

        if now >= next_hover {
            next_hover = now + Duration::from_millis(60);
            hover = s.hover_on && over_desktop(r.hwnd, hover);
        }
        if now >= next_top {
            next_top = now + Duration::from_secs(1);
            r.keep_on_top();
        }
        let zoom = s.zoom * if hover { s.hover_factor } else { 1.0 };

        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        frames += 1;
        let dump_now = match &dump {
            Some(p) if frames == 40 => Some(p.as_str()),
            _ => None,
        };
        if let Err(e) = r.frame(&s, zoom, (pt.x, pt.y), dt, &cursors, dump_now) {
            settings::log(&format!("frame failed, rebuilding: {e}"));
            renderer = None;
            retry_at = Instant::now() + Duration::from_millis(500);
        }
        if dump.is_some() && frames >= 41 {
            return;
        }
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
            WM_HOTKEY => {
                match w.0 as i32 {
                    HK_TOGGLE => toggle(),
                    HK_IN => zoom_by(1.2),
                    HK_OUT => zoom_by(1.0 / 1.2),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAY => {
                match (l.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => ui::show(),
                    WM_RBUTTONUP => tray_menu(h),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                match w.0 & 0xFFFF {
                    CMD_TOGGLE => toggle(),
                    CMD_SETTINGS => ui::show(),
                    CMD_EXIT => PostQuitMessage(0),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                // Rebuild the renderer for the new resolution.
                if settings::get().active {
                    settings::update(|s| s.active = false);
                    settings::update(|s| s.active = true);
                }
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

fn tray_menu(h: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let on = settings::get().active;
        let _ = AppendMenuW(menu, MF_STRING | if on { MF_CHECKED } else { MF_UNCHECKED }, CMD_TOGGLE, w!("Magnifier on\tCtrl+Alt+Z"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("Settings..."));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, w!("Exit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(h);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, Some(0), h, None);
        let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn tray_icon() -> HICON {
    // A magnifying glass, drawn per pixel with soft edges.
    const N: i32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let ring = ((((fx - 13.0).powi(2) + (fy - 13.0).powi(2)).sqrt() - 8.0).abs() - 1.8).max(-1.0);
            // handle: distance to the segment (19,19)-(28,28)
            let (ax, ay, bx, by) = (19.0f32, 19.0f32, 28.0f32, 28.0f32);
            let t = (((fx - ax) * (bx - ax) + (fy - ay) * (by - ay)) / ((bx - ax).powi(2) + (by - ay).powi(2))).clamp(0.0, 1.0);
            let handle = (((fx - (ax + t * (bx - ax))).powi(2) + (fy - (ay + t * (by - ay))).powi(2)).sqrt() - 2.2).max(-1.0);
            let d = ring.min(handle);
            let a = (0.5 - d).clamp(0.0, 1.0);
            let i = ((y * N + x) * 4) as usize;
            px[i] = 255;
            px[i + 1] = 190;
            px[i + 2] = 90;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    unsafe {
        let color = CreateBitmap(N, N, 1, 32, Some(px.as_ptr() as *const _));
        let mask = CreateBitmap(N, N, 1, 1, Some([0u8; 128].as_ptr() as *const _));
        let ii = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
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
