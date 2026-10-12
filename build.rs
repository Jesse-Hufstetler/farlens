#[path = "src/icon_art.rs"]
mod icon_art;

use std::path::PathBuf;

/// A multi-size .ico (uncompressed 32-bit BMP entries) of the app icon.
fn make_ico() -> Vec<u8> {
    const SIZES: [usize; 8] = [16, 20, 24, 32, 40, 48, 64, 128];
    let mut images: Vec<Vec<u8>> = Vec::new();
    for &n in &SIZES {
        let rgba = icon_art::render(n);
        let mut d = Vec::new();
        // BITMAPINFOHEADER; height counts the XOR bitmap and the AND mask.
        for v in [40u32, n as u32, (n * 2) as u32] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(&[0u8; 24]);
        for y in (0..n).rev() {
            for x in 0..n {
                let i = (y * n + x) * 4;
                d.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        d.extend(std::iter::repeat_n(0u8, n.div_ceil(32) * 4 * n));
        images.push(d);
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len();
    for (&n, img) in SIZES.iter().zip(&images) {
        ico.extend_from_slice(&[n as u8, n as u8, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        ico.extend_from_slice(&img);
    }
    ico
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=app.manifest");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = out.join("farlens.ico");
    std::fs::write(&ico, make_ico()).unwrap();

    // Resource 1 is the icon (the exe's icon is the lowest-numbered one); 24 = manifest.
    let slash = |p: PathBuf| p.to_string_lossy().replace('\\', "/");
    let rc = format!("1 ICON \"{}\"\n1 24 \"{}\"\n", slash(ico), slash(manifest_dir.join("app.manifest")));
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).unwrap();
    let _ = embed_resource::compile(rc_path, embed_resource::NONE);
}
