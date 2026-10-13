//! Dev helper: report how many "highlight red" pixels each event image
//! contains, to sanity-check the rect/diff drawing without eyeballing JPEGs.
//! Usage: cargo run --example check_img -- <file.jpg> [more...]

fn main() {
    for p in std::env::args().skip(1) {
        let img = image::open(&p).unwrap().to_rgb8();
        let mut red = 0u32;
        let mut dim = 0u32;
        for px in img.pixels() {
            let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
            if r > 180 && g < 130 && b < 130 {
                red += 1;
            }
            if r < 120 && g < 120 && b < 120 {
                dim += 1;
            }
        }
        println!(
            "{}: {}x{} red={} dim={}",
            p,
            img.width(),
            img.height(),
            red,
            dim
        );
    }
}
