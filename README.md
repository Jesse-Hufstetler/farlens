# farlens

A tray magnifier for when you sit far from the screen. It puts your whole
desktop on a floating 3D plane and points a camera at the mouse.

Windows 10 2004+ / 11. One exe, no installer. Build with `cargo build --release`
(`target/release/farlens.exe`).

## How it works

- **Capture**: DXGI Desktop Duplication copies the desktop into a texture with a
  full mip chain every time it changes.
- **Overlay**: a topmost, click-through window with `WDA_EXCLUDEFROMCAPTURE`, so
  the capture keeps seeing the real desktop under it (no feedback loop). Clicks
  fall through to the real windows.
- **Camera**: the mouse is always dead center. The camera *position* chases the
  mouse with a lag, while it keeps looking at it, so moving the mouse tilts the
  view toward where you're heading. Lag 0 gives a flat magnifier.
- **Zoom** is the camera's z distance (wheel, hotkeys or slider).
- **Warp** is applied before the ray is cast, so it bends the plane itself:
  circular fisheye or rectangular (each side switchable). The center stays at
  full zoom, the edges compress to show more context.
- **Filter**: blocky pixels when magnified (sharp-bilinear, seams anti-aliased),
  trilinear/anisotropic mip filtering when shrunk, like Minecraft. Or smooth.
- **Desktop hover**: optionally zooms out while the mouse is over the wallpaper.
- The real cursor is swapped for an invisible one and redrawn on the plane.

## Use

Tray icon: click = settings, right-click = menu. `Ctrl+Alt+Z` toggles,
`Ctrl+Alt+wheel` or `Ctrl+Alt` `+`/`-` changes zoom. Settings are saved in
`%APPDATA%\farlens\settings.ini`; errors go to `log.txt` beside it.

If the app is killed while active and the cursor stays invisible, run
`farlens.exe --restore-cursors`. `farlens.exe --dump out.bmp` renders one frame
to a file and exits (the overlay is invisible to normal screenshots).

## Known limits / next

- Primary monitor only.
- Apps with custom (non-standard) cursors still show the real cursor at the
  real position.
- Games in exclusive fullscreen and the UAC secure desktop can't be captured.
- Chase-the-mouse (instead of always centered) is not built yet.
- The exe manifest conflicts with Rust's default one at link time (warning);
  DPI awareness is set at runtime instead.
