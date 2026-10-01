//! The real mouse cursor is hidden while the plane is shown, and drawn on the
//! plane instead. Windows has no "hide cursor for everyone" switch, so the
//! standard cursors are swapped for an invisible one (and restored afterwards).
//! Their original images are kept so we can still render them ourselves.

use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub const TEX: i32 = 128;

const IDS: [u16; 16] = [
    32512, 32513, 32514, 32515, 32516, 32631, 32632, 32642, 32643, 32644, 32645, 32646, 32648,
    32649, 32650, 32651,
];

#[derive(Default)]
pub struct SysCursors {
    hidden: bool,
    /// (handle the system now reports, saved copy of the original image)
    map: Vec<(isize, HCURSOR)>,
}

fn ocr(id: u16) -> PCWSTR {
    PCWSTR(id as usize as *const u16)
}

impl SysCursors {
    pub fn hide(&mut self) {
        if self.hidden {
            return;
        }
        unsafe {
            let and = [0xFFu8; 128];
            let xor = [0u8; 128];
            let mut saved = Vec::new();
            for id in IDS {
                if let Ok(orig) = LoadCursorW(None, ocr(id)) {
                    if let Ok(copy) = CopyIcon(HICON(orig.0)) {
                        saved.push((id, HCURSOR(copy.0)));
                    }
                }
            }
            for (id, copy) in saved {
                let Ok(blank) = CreateCursor(None, 0, 0, 32, 32, and.as_ptr() as *const c_void, xor.as_ptr() as *const c_void) else { continue };
                if SetSystemCursor(blank, SYSTEM_CURSOR_ID(id as u32)).is_ok() {
                    if let Ok(now) = LoadCursorW(None, ocr(id)) {
                        self.map.push((now.0 as isize, copy));
                    }
                }
            }
            self.hidden = true;
        }
    }

    pub fn restore(&mut self) {
        if !self.hidden {
            return;
        }
        unsafe {
            let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            for (_, c) in self.map.drain(..) {
                let _ = DestroyIcon(HICON(c.0));
            }
        }
        self.hidden = false;
    }

    /// The cursor whose image we should draw for what the system reports.
    pub fn original(&self, cur: HCURSOR) -> HCURSOR {
        for (now, copy) in &self.map {
            if *now == cur.0 as isize {
                return *copy;
            }
        }
        cur
    }
}

impl Drop for SysCursors {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Restore the system cursors without a `SysCursors` (used by the panic hook
/// and `--restore-cursors`).
pub fn restore_all() {
    unsafe {
        let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
}

pub struct Image {
    pub hot: (i32, i32),
    /// TEX*TEX BGRA, straight (not premultiplied) alpha.
    pub bgra: Vec<u8>,
}

/// Rasterize a cursor. Drawn once over black and once over white; the
/// difference gives the alpha even for old-style mask cursors.
pub fn rasterize(cur: HCURSOR) -> Option<Image> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(HICON(cur.0), &mut ii).ok()?;
        let hot = (ii.xHotspot as i32, ii.yHotspot as i32);
        if !ii.hbmMask.is_invalid() { let _ = DeleteObject(ii.hbmMask.into()); }
        if !ii.hbmColor.is_invalid() { let _ = DeleteObject(ii.hbmColor.into()); }

        let dc = CreateCompatibleDC(None);
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = TEX;
        bi.bmiHeader.biHeight = -TEX;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old = SelectObject(dc, bmp.into());
        let n = (TEX * TEX * 4) as usize;
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, n);

        px.fill(0);
        let _ = DrawIconEx(dc, 0, 0, HICON(cur.0), 0, 0, 0, None, DI_NORMAL);
        let black = px.to_vec();
        px.fill(255);
        let _ = DrawIconEx(dc, 0, 0, HICON(cur.0), 0, 0, 0, None, DI_NORMAL);
        let white = px.to_vec();

        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);

        let mut out = vec![0u8; n];
        for i in (0..n).step_by(4) {
            let mut diff = 0.0f32;
            for c in 0..3 {
                diff += (white[i + c] as f32 - black[i + c] as f32) / 255.0;
            }
            let a = (1.0 - diff / 3.0).clamp(0.0, 1.0);
            if a > 0.004 {
                for c in 0..3 {
                    out[i + c] = (black[i + c] as f32 / a).min(255.0) as u8;
                }
            }
            out[i + 3] = (a * 255.0).round() as u8;
        }
        Some(Image { hot, bgra: out })
    }
}

pub fn current() -> Option<(HCURSOR, bool)> {
    unsafe {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        GetCursorInfo(&mut ci).ok()?;
        Some((ci.hCursor, ci.flags.0 & 1 != 0))
    }
}

#[allow(dead_code)]
pub fn _unused(_: HWND) {}
