//! Desktop capture (DXGI Desktop Duplication) + the overlay window + the
//! full-screen shader pass that draws the desktop on a floating plane.
//!
//! The overlay is excluded from capture (`WDA_EXCLUDEFROMCAPTURE`), so the
//! duplication keeps seeing the real desktop underneath it. That is what makes
//! "magnify the whole screen with a shader" possible without a feedback loop.

use std::ffi::c_void;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::cursor::{self, SysCursors};
use crate::settings::{Settings, Shape};

const SHADER: &str = include_str!("shader.hlsl");
const OVERLAY_CLASS: PCWSTR = w!("FarlensOverlay");
const VFOV_DEG: f32 = 60.0;

pub fn register_overlay_class() {
    unsafe {
        let hi = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: hi.into(),
            lpszClassName: OVERLAY_CLASS,
            hCursor: HCURSOR::default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

unsafe extern "system" fn overlay_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

/// Camera state that persists between frames.
struct Cam {
    px: f32,
    py: f32,
    ax: f32,
    ay: f32,
    sx: f32,
    sy: f32,
    zoom: f32,
}

pub struct Renderer {
    pub hwnd: HWND,
    w: u32,
    h: u32,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    swap: IDXGISwapChain,
    rtv: ID3D11RenderTargetView,
    dup: IDXGIOutputDuplication,
    screen: ID3D11Texture2D,
    screen_srv: ID3D11ShaderResourceView,
    cur_tex: ID3D11Texture2D,
    cur_srv: ID3D11ShaderResourceView,
    cur_handle: isize,
    cur_hot: (i32, i32),
    cbuf: ID3D11Buffer,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    aniso: ID3D11SamplerState,
    lin: ID3D11SamplerState,
    cam: Cam,
    got_frame: bool,
    blend: f32,
    /// Set to start easing back to the plain desktop; done turns true when that finished.
    pub closing: bool,
    pub done: bool,
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.ctx.ClearState();
            self.ctx.Flush();
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        let r = D3DCompile(
            SHADER.as_ptr() as *const c_void,
            SHADER.len(),
            s!("shader.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut err),
        );
        if let Err(e) = r {
            if let Some(b) = err {
                let msg = std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize());
                return Err(Error::new(e.code(), String::from_utf8_lossy(msg).to_string()));
            }
            return Err(e);
        }
        Ok(code.unwrap())
    }
}

impl Renderer {
    /// `affinity_exclude`: hide our own window from capture (always true outside tests).
    pub fn new() -> Result<Renderer> {
        unsafe {
            // Find the output that holds the primary monitor, and the adapter that owns it.
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut found: Option<(IDXGIAdapter1, IDXGIOutput, RECT)> = None;
            let mut ai = 0;
            while let Ok(ad) = factory.EnumAdapters1(ai) {
                let mut oi = 0;
                while let Ok(out) = ad.EnumOutputs(oi) {
                    let d = out.GetDesc()?;
                    let r = d.DesktopCoordinates;
                    if r.left <= 0 && r.top <= 0 && r.right > 0 && r.bottom > 0 && d.AttachedToDesktop.as_bool() {
                        found = Some((ad.clone(), out, r));
                    }
                    oi += 1;
                }
                ai += 1;
            }
            let (adapter, output, rect) = found.ok_or_else(|| Error::new(E_FAIL, "no primary output"))?;
            let w = (rect.right - rect.left) as u32;
            let h = (rect.bottom - rect.top) as u32;

            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            let base: IDXGIAdapter = adapter.cast()?;
            D3D11CreateDevice(
                &base,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device = device.unwrap();
            let ctx = ctx.unwrap();

            // Overlay window: topmost, click-through, never activated, invisible to capture.
            let hi = GetModuleHandleW(None)?;
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                OVERLAY_CLASS,
                w!("Farlens"),
                WS_POPUP,
                rect.left,
                rect.top,
                w as i32,
                h as i32,
                None,
                None,
                Some(hi.into()),
                None,
            )?;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            if std::env::var_os("FARLENS_NO_EXCLUDE").is_none() {
                SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(0x11))?; // WDA_EXCLUDEFROMCAPTURE
            }

            let desc = DXGI_SWAP_CHAIN_DESC {
                BufferDesc: DXGI_MODE_DESC {
                    Width: w,
                    Height: h,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    ..Default::default()
                },
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                OutputWindow: hwnd,
                Windowed: true.into(),
                SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
                ..Default::default()
            };
            let mut swap: Option<IDXGISwapChain> = None;
            factory.CreateSwapChain(&device, &desc, &mut swap).ok()?;
            let swap = swap.unwrap();
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);

            let back: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut rtv: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            let rtv = rtv.unwrap();

            let out1: IDXGIOutput1 = output.cast()?;
            let dup = out1.DuplicateOutput(&device)?;

            // Desktop copy with a full mip chain (so zooming out doesn't alias).
            let td = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 0,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
                ..Default::default()
            };
            let mut screen: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&td, None, Some(&mut screen))?;
            let screen = screen.unwrap();
            let mut screen_srv: Option<ID3D11ShaderResourceView> = None;
            device.CreateShaderResourceView(&screen, None, Some(&mut screen_srv))?;

            let cd = D3D11_TEXTURE2D_DESC {
                Width: cursor::TEX as u32,
                Height: cursor::TEX as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut cur_tex: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&cd, None, Some(&mut cur_tex))?;
            let cur_tex = cur_tex.unwrap();
            let mut cur_srv: Option<ID3D11ShaderResourceView> = None;
            device.CreateShaderResourceView(&cur_tex, None, Some(&mut cur_srv))?;

            let bd = D3D11_BUFFER_DESC {
                ByteWidth: 10 * 16,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut cbuf: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&bd, None, Some(&mut cbuf))?;

            let vsb = compile(s!("vs"), s!("vs_5_0"))?;
            let psb = compile(s!("ps"), s!("ps_5_0"))?;
            let vsc = std::slice::from_raw_parts(vsb.GetBufferPointer() as *const u8, vsb.GetBufferSize());
            let psc = std::slice::from_raw_parts(psb.GetBufferPointer() as *const u8, psb.GetBufferSize());
            let mut vs: Option<ID3D11VertexShader> = None;
            let mut ps: Option<ID3D11PixelShader> = None;
            device.CreateVertexShader(vsc, None, Some(&mut vs))?;
            device.CreatePixelShader(psc, None, Some(&mut ps))?;

            let mk = |filter: D3D11_FILTER, aniso: u32| -> Result<ID3D11SamplerState> {
                let sd = D3D11_SAMPLER_DESC {
                    Filter: filter,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxAnisotropy: aniso,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    MinLOD: 0.0,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                };
                let mut s: Option<ID3D11SamplerState> = None;
                device.CreateSamplerState(&sd, Some(&mut s))?;
                Ok(s.unwrap())
            };
            let aniso = mk(D3D11_FILTER_ANISOTROPIC, 16)?;
            let lin = mk(D3D11_FILTER_MIN_MAG_MIP_LINEAR, 1)?;

            ShowWindow(hwnd, SW_SHOWNA).ok().ok();
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), rect.left, rect.top, w as i32, h as i32, SWP_NOACTIVATE);

            Ok(Renderer {
                hwnd,
                w,
                h,
                device,
                ctx,
                swap,
                rtv,
                dup,
                screen,
                screen_srv: screen_srv.unwrap(),
                cur_tex,
                cur_srv: cur_srv.unwrap(),
                cur_handle: 0,
                cur_hot: (0, 0),
                cbuf: cbuf.unwrap(),
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                aniso,
                lin,
                cam: Cam { px: 0.0, py: 0.0, ax: 0.0, ay: 0.0, sx: 0.0, sy: 0.0, zoom: 0.0 },
                got_frame: false,
                blend: 0.0,
                closing: false,
                done: false,
            })
        }
    }

    pub fn keep_on_top(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Pull the newest desktop image, if there is one.
    fn grab(&mut self) -> Result<()> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            let timeout = if self.got_frame { 0 } else { 50 };
            match self.dup.AcquireNextFrame(timeout, &mut info, &mut res) {
                Ok(()) => {
                    if let Some(res) = res {
                        let tex: ID3D11Texture2D = res.cast()?;
                        self.ctx.CopySubresourceRegion(&self.screen, 0, 0, 0, 0, &tex, 0, None);
                        self.ctx.GenerateMips(&self.screen_srv);
                        self.got_frame = true;
                    }
                    let _ = self.dup.ReleaseFrame();
                    Ok(())
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    // Duplication only reports changes; on a still desktop it can stay silent
                    // for a long time, so take the first image with GDI instead.
                    if !self.got_frame {
                        self.grab_gdi();
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    }

    fn grab_gdi(&mut self) {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            let sdc = GetDC(None);
            let dc = CreateCompatibleDC(Some(sdc));
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = self.w as i32;
            bi.bmiHeader.biHeight = -(self.h as i32);
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            let mut bits: *mut c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                let old = SelectObject(dc, bmp.into());
                if BitBlt(dc, 0, 0, self.w as i32, self.h as i32, Some(sdc), 0, 0, SRCCOPY | CAPTUREBLT).is_ok() {
                    self.ctx.UpdateSubresource(&self.screen, 0, None, bits as *const c_void, self.w * 4, 0);
                    self.ctx.GenerateMips(&self.screen_srv);
                    self.got_frame = true;
                }
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
            }
            let _ = DeleteDC(dc);
            ReleaseDC(None, sdc);
        }
    }

    fn upload_cursor(&mut self, cursors: &SysCursors) {
        let Some((h, _)) = cursor::current() else { return };
        if h.0 as isize == self.cur_handle {
            return;
        }
        let src = cursors.original(h);
        if let Some(img) = cursor::rasterize(src) {
            unsafe {
                self.ctx.UpdateSubresource(&self.cur_tex, 0, None, img.bgra.as_ptr() as *const c_void, (cursor::TEX * 4) as u32, 0);
            }
            self.cur_hot = img.hot;
            self.cur_handle = h.0 as isize;
        }
    }

    /// Draw one frame. `mouse` is in desktop pixels. With `dump`, the finished
    /// frame is also written to that BMP path.
    pub fn frame(&mut self, s: &Settings, target_zoom: f32, mouse: (i32, i32), dt: f32, cursors: &SysCursors, dump: Option<&str>) -> Result<()> {
        self.grab()?;
        if !self.got_frame {
            return Ok(());
        }
        self.upload_cursor(cursors);

        let (wf, hf) = (self.w as f32, self.h as f32);
        let aspect = wf / hf;
        let tan_half = (VFOV_DEG.to_radians() * 0.5).tan();
        let f_px = (hf * 0.5) / tan_half;
        let (mx, my) = (mouse.0 as f32 + 0.5, mouse.1 as f32 + 0.5);

        // Opening and closing ease the whole view between "plain 1:1 screen" (zoom 1,
        // camera centered, no warp) and the live camera. `blend` runs 0..1.
        let dir = if self.closing { -1.0 } else { 1.0 };
        self.blend = (self.blend + dir * dt / 0.45).clamp(0.0, 1.0);
        let blend = self.blend;
        let (cx, cy) = (wf * 0.5, hf * 0.5);
        let (tx, ty) = if self.closing { (cx, cy) } else { (mx, my) };
        let zoom_goal = if self.closing { 1.0 } else { target_zoom };
        let slow = 1.0 - blend; // extra smoothing while opening/closing
        // Opening is gentle. Closing gets quicker and quicker (tau -> 0) so it always lands exactly on 1:1.
        let closing = self.closing;
        let tau = |user: f32| if closing { user.max(0.12) * blend * blend } else { user.max(0.2 * slow) };

        // Camera: zoom eases in log space; x,y chase the mouse; the aim chases it too if asked.
        let c = &mut self.cam;
        if c.zoom <= 0.0 {
            c.zoom = 1.0;
            c.px = cx;
            c.py = cy;
            c.ax = cx;
            c.ay = cy;
        }
        let zt = if closing { 0.10 * blend * blend } else { 0.10 + 0.15 * slow };
        let kz = if zt <= 1e-4 { 1.0 } else { 1.0 - (-dt / zt).exp() };
        c.zoom = (c.zoom.ln() + (zoom_goal.ln() - c.zoom.ln()) * kz).exp();
        let d = f_px / c.zoom;
        // 1) Input smoothing: the mouse as Farlens perceives it (kills jerky/stepped motion).
        let sm = tau(s.smooth);
        if sm > 0.001 {
            let a = 1.0 - (-dt / sm).exp();
            c.sx += (tx - c.sx) * a;
            c.sy += (ty - c.sy) * a;
        } else {
            c.sx = tx;
            c.sy = ty;
        }
        // 2) Camera chase: the camera position follows the smoothed mouse with its own lag.
        let lag = tau(s.lag);
        if lag > 0.001 {
            let a = 1.0 - (-dt / lag).exp();
            c.px += (c.sx - c.px) * a;
            c.py += (c.sy - c.py) * a;
        } else {
            c.px = c.sx;
            c.py = c.sy;
        }
        // 3) Aim: on the smoothed mouse (it stays centered and the lagging camera leans toward
        //    it), or, with "chase the mouse", on the camera itself (flat view, mouse may drift).
        if s.aim_chase {
            c.ax = c.px;
            c.ay = c.py;
        } else {
            c.ax = c.sx;
            c.ay = c.sy;
        }
        let (mut ox, mut oy) = (c.ax - c.px, c.ay - c.py);
        let max_off = d * s.max_tilt.to_radians().tan();
        let len = (ox * ox + oy * oy).sqrt();
        if len > max_off {
            ox *= max_off / len;
            oy *= max_off / len;
            c.px = c.ax - ox;
            c.py = c.ay - oy;
        }
        if closing && blend <= 0.0 {
            // Fully eased out: land exactly on the plain desktop.
            c.zoom = 1.0;
            c.px = cx;
            c.py = cy;
            c.ax = cx;
            c.ay = cy;
            c.sx = cx;
            c.sy = cy;
            self.done = true;
        }
        let warp_k = blend * blend * (3.0 - 2.0 * blend);
        // Basis (plane: x right, y down, z toward the viewer).
        let fwd = norm([ox, oy, -d]);
        let right = norm(cross([0.0, -1.0, 0.0], fwd));
        let up = cross(fwd, right);
        let (vis, _) = cursor::current().map(|(_, v)| ((v as u8) as f32, 0)).unwrap_or((0.0, 0));
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        let cb: [[f32; 4]; 10] = [
            [wf, hf, wf, hf],
            [c.px, c.py, d, tan_half],
            [right[0], right[1], right[2], aspect],
            [up[0], up[1], up[2], 0.0],
            [fwd[0], fwd[1], fwd[2], b(s.warp_on)],
            [mx, my, vis, 0.0],
            [self.cur_hot.0 as f32, self.cur_hot.1 as f32, if s.warp_shape == Shape::Rect { 1.0 } else { 0.0 }, s.warp_radius],
            [s.warp_amount * warp_k, b(s.warp_top), b(s.warp_bottom), b(s.warp_left)],
            [b(s.warp_right), b(s.bg_desktop), 0.0, 0.0],
            [s.shadow, s.shadow_size, s.bg_level, 0.0],
        ];

        unsafe {
            self.ctx.UpdateSubresource(&self.cbuf, 0, None, cb.as_ptr() as *const c_void, 0, 0);
            self.ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: wf, Height: hf, MinDepth: 0.0, MaxDepth: 1.0 }]));
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(&self.ps, None);
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.cbuf.clone())]));
            self.ctx.PSSetShaderResources(0, Some(&[Some(self.screen_srv.clone()), Some(self.cur_srv.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.aniso.clone()), Some(self.lin.clone())]));
            self.ctx.Draw(3, 0);

            if let Some(path) = dump {
                self.dump_bmp(path)?;
            }
            let hr = self.swap.Present(1, DXGI_PRESENT(0));
            if hr.is_err() {
                return Err(Error::from(hr));
            }
        }
        Ok(())
    }

    unsafe fn dump_bmp(&self, path: &str) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let mut td = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut td);
            td.Usage = D3D11_USAGE_STAGING;
            td.BindFlags = 0;
            td.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            td.MiscFlags = 0;
            let mut st: Option<ID3D11Texture2D> = None;
            self.device.CreateTexture2D(&td, None, Some(&mut st))?;
            let st = st.unwrap();
            self.ctx.CopyResource(&st, &back);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let (w, h) = (self.w as usize, self.h as usize);
            let mut data = Vec::with_capacity(54 + w * h * 4);
            let size = (54 + w * h * 4) as u32;
            data.extend_from_slice(b"BM");
            data.extend_from_slice(&size.to_le_bytes());
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(&54u32.to_le_bytes());
            data.extend_from_slice(&40u32.to_le_bytes());
            data.extend_from_slice(&(w as i32).to_le_bytes());
            data.extend_from_slice(&(-(h as i32)).to_le_bytes());
            data.extend_from_slice(&1u16.to_le_bytes());
            data.extend_from_slice(&32u16.to_le_bytes());
            data.extend_from_slice(&[0; 24]);
            for y in 0..h {
                let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w * 4);
                data.extend_from_slice(row);
            }
            self.ctx.Unmap(&st, 0);
            let _ = std::fs::write(path, data);
            Ok(())
        }
    }
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
