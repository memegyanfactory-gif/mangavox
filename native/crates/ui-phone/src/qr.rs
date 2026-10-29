//! QR code drawn with GPUI: dark modules on a light square with the standard 4-module quiet zone,
//! painted in one `canvas` (each horizontal run of dark modules is one quad), so a 260 px code
//! costs a few hundred quads and no elements.
use gpui_kit::*;
use mv_ui::theme::{radius, size};
use mv_ui::Tokens;
use qrcode::{Color, EcLevel, QrCode};

/// Modules of light margin around the code (ISO/IEC 18004 asks for 4).
const QUIET: usize = 4;

/// The module matrix for `data`: (width, row-major dark flags), or `None` when it does not fit.
pub fn modules(data: &str) -> Option<(usize, Vec<bool>)> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M).ok()?;
    let w = code.width();
    let dark = code.to_colors().into_iter().map(|c| c == Color::Dark).collect();
    Some((w, dark))
}

/// Horizontal runs of dark modules per row: (row, first column, length).
fn runs(w: usize, dark: &[bool]) -> Vec<(usize, usize, usize)> {
    let mut out = vec![];
    for y in 0..w {
        let row = &dark[y * w..(y + 1) * w];
        let mut x = 0;
        while x < w {
            if row[x] {
                let start = x;
                while x < w && row[x] {
                    x += 1;
                }
                out.push((y, start, x - start));
            } else {
                x += 1;
            }
        }
    }
    out
}

/// A scannable QR code for `data`, about `size_px` square (the module size is rounded to whole
/// pixels so runs meet without seams).
pub fn qr_code(data: &str, size_px: f32) -> AnyElement {
    let Some((w, dark)) = modules(data) else {
        return div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(size_px))
            .rounded(px(radius::LG))
            .bg(Tokens::well())
            .text_size(px(size::SM))
            .text_color(Tokens::text_3())
            .child("This address is too long for a QR code")
            .into_any_element();
    };
    let total = w + 2 * QUIET;
    let module = (size_px / total as f32).floor().max(1.);
    let side = module * total as f32;
    let runs = runs(w, &dark);
    let (light, ink) = (Tokens::text_1(), Tokens::base());
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, Hsla::from(light)));
            let o = bounds.origin;
            for &(y, x, len) in &runs {
                let at = point(o.x + px((QUIET + x) as f32 * module), o.y + px((QUIET + y) as f32 * module));
                window.paint_quad(fill(Bounds::new(at, gpui_kit::size(px(len as f32 * module), px(module))), Hsla::from(ink)));
            }
        },
    )
    .flex_none()
    .size(px(side))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{modules, runs};

    #[test]
    fn a_pairing_url_encodes_to_a_square_matrix() {
        let url = "http://192.168.1.20:8790/pair?code=Zm9vYmFyYmF6cXV4cXV1eHh5enp5MTIzNDU2Nzg5MGFi";
        let (w, dark) = modules(url).expect("fits");
        assert_eq!(dark.len(), w * w);
        // Versions are 21 + 4k modules wide.
        assert!(w >= 21 && (w - 21) % 4 == 0, "{w}");
        // The top-left finder pattern starts with seven dark modules.
        assert!(dark[..7].iter().all(|d| *d));
        // Every dark module is covered by exactly one run.
        let covered: usize = runs(w, &dark).iter().map(|r| r.2).sum();
        assert_eq!(covered, dark.iter().filter(|d| **d).count());
    }
}
