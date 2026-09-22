//! Dev helper: write two synthetic "before"/"after" JPEGs (640x360 grey
//! scene; a red box moves between them) for exercising --selftest and the
//! web server locally.
//!
//! Usage: cargo run --example gen_pairs -- <out-before.jpg> <out-after.jpg>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (before_path, after_path) = match args.as_slice() {
        [_, b, a] => (b.clone(), a.clone()),
        _ => {
            eprintln!("usage: gen_pairs <before.jpg> <after.jpg>");
            std::process::exit(2);
        }
    };

    let base = |x: u32| -> image::RgbImage {
        let mut img = image::RgbImage::from_pixel(640, 360, image::Rgb([118, 122, 130]));
        // Some static texture so the scene is not flat.
        for (px, py, p) in img.enumerate_pixels_mut() {
            let v = (118u16 + (px / 17) as u16 * 3 + (py / 23) as u16 * 5) as u8;
            *p = image::Rgb([v, v.wrapping_add(6), v.wrapping_add(2)]);
        }
        // Table-ish darker band.
        for (px, py, p) in img.enumerate_pixels_mut() {
            if py > 240 && py < 300 {
                let d = (py - 240) as u8;
                *p = image::Rgb([90 - d / 3, 96 - d / 3, 100 - d / 3]);
            }
        }
        // Red "chair".
        for py in 210..270 {
            for px in x..x + 60 {
                *img.get_pixel_mut(px, py) = image::Rgb([190, 40, 30]);
            }
        }
        img
    };

    let enc = |p: &str, img: &image::RgbImage| {
        let f = std::fs::File::create(p).unwrap();
        let mut w = std::io::BufWriter::new(f);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 82)
            .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
    };
    enc(&before_path, &base(80));
    enc(&after_path, &base(400));
    eprintln!("wrote {before_path} and {after_path}");
}
