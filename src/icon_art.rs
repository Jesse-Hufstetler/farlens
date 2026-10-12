//! The app icon, drawn per pixel: a round lens with a fisheye-warped yellow grid,
//! the center cell magnified. Included by build.rs to bake the .ico into the exe.

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// Color and coverage at (u, v) in [-1, 1]^2, y down.
fn shade(u: f32, v: f32) -> (f32, f32, f32, f32) {
    const OUTER: f32 = 0.95;
    const RIM: f32 = 0.80;
    let rr = (u * u + v * v).sqrt();
    if rr > OUTER {
        return (0.0, 0.0, 0.0, 0.0);
    }
    if rr > RIM {
        let t = (v + 1.0) * 0.5;
        return (lerp(1.0, 0.72, t), lerp(0.96, 0.52, t), lerp(0.80, 0.10, t), 1.0);
    }
    // Inside the lens: a unit-disc coordinate, and the source-plane coordinate it shows.
    let (px, py) = (u / RIM, v / RIM);
    let r2 = px * px + py * py;
    let k = 1.5;
    let (qx, qy) = (px * (1.0 + k * r2), py * (1.0 + k * r2));
    let scale = 1.0 + 2.0 * k * r2; // roughly how much plane one lens unit covers
    let hw = 0.045 * scale;
    let near = |q: f32| ((q - q.floor()) - 0.5).abs() < hw;

    let t = (v + 1.0) * 0.5;
    let glow = 0.08 * (1.0 - r2);
    let mut c = (lerp(0.20, 0.10, t) + glow * 1.5, lerp(0.14, 0.07, t) + glow, lerp(0.03, 0.02, t) + glow * 0.3);
    if qx.abs() < 0.5 && qy.abs() < 0.5 {
        c = (0.98, 0.72, 0.08);
    }
    if near(qx) || near(qy) {
        c = (1.0, 0.90, 0.35);
    }
    // Specular highlight, upper left.
    let (hx, hy) = (px + 0.38, (py + 0.5) * 1.6);
    let hd = (hx * hx + hy * hy).sqrt();
    if hd < 0.28 {
        let a = 0.25 * (1.0 - hd / 0.28);
        c = (lerp(c.0, 1.0, a), lerp(c.1, 1.0, a), lerp(c.2, 1.0, a));
    }
    (c.0, c.1, c.2, 1.0)
}

/// `n` x `n` straight (non-premultiplied) RGBA.
pub fn render(n: usize) -> Vec<u8> {
    let ss = if n <= 32 { 4 } else if n <= 64 { 3 } else { 2 };
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let u = ((x as f32 + (sx as f32 + 0.5) / ss as f32) / n as f32) * 2.0 - 1.0;
                    let v = ((y as f32 + (sy as f32 + 0.5) / ss as f32) / n as f32) * 2.0 - 1.0;
                    let (cr, cg, cb, ca) = shade(u, v);
                    r += cr * ca;
                    g += cg * ca;
                    b += cb * ca;
                    a += ca;
                }
            }
            if a > 0.0 {
                let i = (y * n + x) * 4;
                out[i] = (r / a * 255.0).round() as u8;
                out[i + 1] = (g / a * 255.0).round() as u8;
                out[i + 2] = (b / a * 255.0).round() as u8;
                out[i + 3] = (a / (ss * ss) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}
