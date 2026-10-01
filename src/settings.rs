//! Shared settings, persisted as `key=value` lines in `%APPDATA%\farlens\settings.ini`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    /// Blocky when magnified, mip-filtered when shrunk (the Minecraft look).
    Pixelated,
    /// Bilinear when magnified, mip-filtered when shrunk.
    Smooth,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    Circle,
    Rect,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub active: bool,
    pub zoom: f32,
    /// Seconds the camera takes to catch up with the mouse. 0 = locked on.
    pub lag: f32,
    /// Largest angle (degrees) the camera may tilt away from the plane normal.
    pub max_tilt: f32,
    /// Let the view's center chase the mouse too, so the mouse can drift off-center.
    pub aim_chase: bool,
    pub aim_lag: f32,
    pub filter: Filter,
    pub hide_cursor: bool,
    pub warp_on: bool,
    pub warp_shape: Shape,
    pub warp_radius: f32,
    pub warp_amount: f32,
    pub warp_top: bool,
    pub warp_bottom: bool,
    pub warp_left: bool,
    pub warp_right: bool,
    pub hover_on: bool,
    /// Zoom multiplier applied while the mouse is over the bare desktop.
    pub hover_factor: f32,
}

impl Settings {
    pub const fn new() -> Self {
        Settings {
            active: true,
            zoom: 2.0,
            lag: 0.18,
            max_tilt: 35.0,
            aim_chase: false,
            aim_lag: 0.25,
            filter: Filter::Pixelated,
            hide_cursor: true,
            warp_on: false,
            warp_shape: Shape::Circle,
            warp_radius: 0.6,
            warp_amount: 1.5,
            warp_top: true,
            warp_bottom: true,
            warp_left: true,
            warp_right: true,
            hover_on: false,
            hover_factor: 0.5,
        }
    }

    fn pairs(&self) -> Vec<(&'static str, String)> {
        let b = |v: bool| (v as u8).to_string();
        vec![
            ("active", b(self.active)),
            ("zoom", self.zoom.to_string()),
            ("lag", self.lag.to_string()),
            ("max_tilt", self.max_tilt.to_string()),
            ("aim_chase", b(self.aim_chase)),
            ("aim_lag", self.aim_lag.to_string()),
            ("filter", ((self.filter == Filter::Smooth) as u8).to_string()),
            ("hide_cursor", b(self.hide_cursor)),
            ("warp_on", b(self.warp_on)),
            ("warp_shape", ((self.warp_shape == Shape::Rect) as u8).to_string()),
            ("warp_radius", self.warp_radius.to_string()),
            ("warp_amount", self.warp_amount.to_string()),
            ("warp_top", b(self.warp_top)),
            ("warp_bottom", b(self.warp_bottom)),
            ("warp_left", b(self.warp_left)),
            ("warp_right", b(self.warp_right)),
            ("hover_on", b(self.hover_on)),
            ("hover_factor", self.hover_factor.to_string()),
        ]
    }

    fn set(&mut self, k: &str, v: &str) {
        let f = || v.parse::<f32>().ok();
        let b = || v.trim() == "1" || v.trim() == "true";
        match k {
            "active" => self.active = b(),
            "zoom" => if let Some(x) = f() { self.zoom = x },
            "lag" => if let Some(x) = f() { self.lag = x },
            "max_tilt" => if let Some(x) = f() { self.max_tilt = x },
            "aim_chase" => self.aim_chase = b(),
            "aim_lag" => if let Some(x) = f() { self.aim_lag = x },
            "filter" => self.filter = if b() { Filter::Smooth } else { Filter::Pixelated },
            "hide_cursor" => self.hide_cursor = b(),
            "warp_on" => self.warp_on = b(),
            "warp_shape" => self.warp_shape = if b() { Shape::Rect } else { Shape::Circle },
            "warp_radius" => if let Some(x) = f() { self.warp_radius = x },
            "warp_amount" => if let Some(x) = f() { self.warp_amount = x },
            "warp_top" => self.warp_top = b(),
            "warp_bottom" => self.warp_bottom = b(),
            "warp_left" => self.warp_left = b(),
            "warp_right" => self.warp_right = b(),
            "hover_on" => self.hover_on = b(),
            "hover_factor" => if let Some(x) = f() { self.hover_factor = x },
            _ => {}
        }
    }

    /// Fix anything a hand-edited file could break.
    pub fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(0.25, 32.0);
        self.lag = self.lag.clamp(0.0, 3.0);
        self.max_tilt = self.max_tilt.clamp(1.0, 80.0);
        self.aim_lag = self.aim_lag.clamp(0.0, 3.0);
        self.warp_radius = self.warp_radius.clamp(0.0, 2.0);
        self.warp_amount = self.warp_amount.clamp(0.0, 10.0);
        self.hover_factor = self.hover_factor.clamp(0.05, 1.0);
    }
}

static CURRENT: Mutex<Settings> = Mutex::new(Settings::new());
static DIRTY: Mutex<Option<Instant>> = Mutex::new(None);

pub fn get() -> Settings {
    CURRENT.lock().unwrap().clone()
}

/// Change the settings. Never call this while the caller holds a UI borrow that
/// the UI's own handlers also take; the lock here is only held for `f`.
pub fn update(f: impl FnOnce(&mut Settings)) {
    {
        let mut s = CURRENT.lock().unwrap();
        f(&mut s);
        s.clamp();
    }
    *DIRTY.lock().unwrap() = Some(Instant::now());
}

fn path() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("APPDATA")?).join("farlens");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("settings.ini"))
}

pub fn load() {
    let Some(p) = path() else { return };
    let Ok(text) = std::fs::read_to_string(p) else { return };
    let mut s = Settings::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            s.set(k.trim(), v);
        }
    }
    s.clamp();
    *CURRENT.lock().unwrap() = s;
}

/// Write the settings if they changed and have been quiet for a moment.
pub fn save_if_due() {
    let due = matches!(*DIRTY.lock().unwrap(), Some(t) if t.elapsed().as_millis() > 400);
    if !due {
        return;
    }
    *DIRTY.lock().unwrap() = None;
    let Some(p) = path() else { return };
    let mut out = String::new();
    for (k, v) in get().pairs() {
        out.push_str(&format!("{k}={v}\n"));
    }
    let _ = std::fs::write(p, out);
}

pub fn log(msg: &str) {
    if let Some(p) = path() {
        let p = p.with_file_name("log.txt");
        let prev = std::fs::read_to_string(&p).unwrap_or_default();
        let keep: String = prev.lines().rev().take(200).collect::<Vec<_>>().into_iter().rev().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(p, format!("{keep}{msg}\n"));
    }
}
