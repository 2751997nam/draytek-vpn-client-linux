//! Coloured VPN shield icons as ARGB32 pixmaps for the system tray.
//!
//! Shared by both indicators — the NetworkManager tray and the standalone app's
//! tray — so the VPN presents one recognisable mark wherever it appears, rather
//! than a themed network glyph that vanishes among the system's own tray icons.
//!
//! Each state is rendered at several sizes so the host can pick what fits its
//! panel. Shapes are defined analytically in the unit square and sampled with
//! 4×4 supersampling, which keeps the silhouette smooth at every size and lets
//! the glyph be clipped to the shield by construction.
//!
//! Pixel data is straight (non-premultiplied) ARGB32, as the StatusNotifierItem
//! spec requires.

use std::sync::LazyLock;

/// Sizes emitted for every state; hosts choose the closest fit.
const SIZES: [i32; 3] = [22, 32, 44];

/// Supersamples per axis — 16 coverage samples per pixel.
const SUPERSAMPLES: u32 = 4;

/// Connected — green shield with a checkmark.
pub static CONNECTED: LazyLock<Vec<ksni::Icon>> = LazyLock::new(|| {
    icon_set(Design {
        rgb: [0x4C, 0xAF, 0x50],
        glyph: Glyph::Check,
    })
});

/// Disconnected — red shield with a cross.
pub static DISCONNECTED: LazyLock<Vec<ksni::Icon>> = LazyLock::new(|| {
    icon_set(Design {
        rgb: [0xF4, 0x43, 0x36],
        glyph: Glyph::Cross,
    })
});

/// Connecting or disconnecting — amber shield with an ellipsis.
pub static CONNECTING: LazyLock<Vec<ksni::Icon>> = LazyLock::new(|| {
    icon_set(Design {
        rgb: [0xFF, 0x98, 0x00],
        glyph: Glyph::Dots,
    })
});

enum Glyph {
    Check,
    Cross,
    Dots,
}

struct Design {
    rgb: [u8; 3],
    glyph: Glyph,
}

fn icon_set(design: Design) -> Vec<ksni::Icon> {
    SIZES.iter().map(|&size| render(&design, size)).collect()
}

fn render(design: &Design, size: i32) -> ksni::Icon {
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];
    let samples = (SUPERSAMPLES * SUPERSAMPLES) as f32;
    let step = 1.0 / SUPERSAMPLES as f32;

    for py in 0..s {
        for px in 0..s {
            // Coverage of the shield, and of the glyph within it. Counting the
            // glyph only where the shield already covers clips it to the
            // silhouette, so a stroke can never spill over the edge.
            let mut shield = 0.0f32;
            let mut glyph = 0.0f32;

            for sy in 0..SUPERSAMPLES {
                for sx in 0..SUPERSAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) * step) / s as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) * step) / s as f32;
                    if in_shield(x, y) {
                        shield += 1.0;
                        if in_glyph(&design.glyph, x, y) {
                            glyph += 1.0;
                        }
                    }
                }
            }

            if shield == 0.0 {
                continue;
            }

            let alpha = shield / samples;
            let white = glyph / shield;
            let blend = |c: u8| (c as f32 * (1.0 - white) + 255.0 * white).round() as u8;
            let argb = [
                (alpha * 255.0).round() as u8,
                blend(design.rgb[0]),
                blend(design.rgb[1]),
                blend(design.rgb[2]),
            ];
            let idx = (py * s + px) * 4;
            data[idx..idx + 4].copy_from_slice(&argb);
        }
    }

    ksni::Icon {
        width: size,
        height: size,
        data,
    }
}

// ── Shapes, in the unit square (x right, y down) ─────────────────────────────

const SHIELD_TOP: f32 = 0.07;
const SHIELD_BOTTOM: f32 = 0.95;
const SHIELD_HALF: f32 = 0.36;
/// Fraction of the height over which the top corners round off.
const SHOULDER: f32 = 0.18;
/// Fraction of the height at which the sides begin curving to the point.
const WAIST: f32 = 0.55;

/// Shield silhouette: rounded top corners, straight sides, and an elliptical
/// taper to a rounded point at the bottom.
fn in_shield(x: f32, y: f32) -> bool {
    if !(SHIELD_TOP..=SHIELD_BOTTOM).contains(&y) {
        return false;
    }

    let t = (y - SHIELD_TOP) / (SHIELD_BOTTOM - SHIELD_TOP);
    let half_width = if t < SHOULDER {
        // Quarter-ellipse corners: narrower at the very top edge.
        let k = (SHOULDER - t) / SHOULDER;
        let corner = SHIELD_HALF * 0.30;
        SHIELD_HALF - corner * (1.0 - (1.0 - k * k).max(0.0).sqrt())
    } else if t <= WAIST {
        SHIELD_HALF
    } else {
        let k = (t - WAIST) / (1.0 - WAIST);
        SHIELD_HALF * (1.0 - k * k).max(0.0).sqrt()
    };

    (x - 0.5).abs() <= half_width
}

/// Where the glyph sits, and how large it is relative to the icon.
const GLYPH_CENTRE: (f32, f32) = (0.5, 0.44);
const GLYPH_SCALE: f32 = 0.30;
/// Stroke half-width for the checkmark. The cross is drawn slightly thinner:
/// its two strokes overlap at the centre, which reads heavier at the same
/// weight.
const STROKE: f32 = 0.19;
const CROSS_STROKE: f32 = 0.155;

fn in_glyph(glyph: &Glyph, x: f32, y: f32) -> bool {
    let gx = (x - GLYPH_CENTRE.0) / GLYPH_SCALE;
    let gy = (y - GLYPH_CENTRE.1) / GLYPH_SCALE;

    match glyph {
        Glyph::Check => {
            near_segment(gx, gy, (-0.55, 0.00), (-0.15, 0.40), STROKE)
                || near_segment(gx, gy, (-0.15, 0.40), (0.58, -0.42), STROKE)
        }
        Glyph::Cross => {
            near_segment(gx, gy, (-0.43, -0.43), (0.43, 0.43), CROSS_STROKE)
                || near_segment(gx, gy, (0.43, -0.43), (-0.43, 0.43), CROSS_STROKE)
        }
        Glyph::Dots => [-0.52f32, 0.0, 0.52]
            .iter()
            .any(|&dx| (gx - dx).hypot(gy) <= 0.17),
    }
}

/// Whether (px, py) lies within `r` of the segment `a`–`b` — a round-capped
/// stroke.
fn near_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32), r: f32) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 0.0 {
        (((px - a.0) * dx + (py - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - (a.0 + t * dx)).hypot(py - (a.1 + t * dy)) <= r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(icon: &ksni::Icon, x: usize, y: usize) -> u8 {
        icon.data[(y * icon.width as usize + x) * 4]
    }

    #[test]
    fn every_state_renders_all_sizes() {
        for set in [&*CONNECTED, &*DISCONNECTED, &*CONNECTING] {
            assert_eq!(set.len(), SIZES.len());
            for (icon, &size) in set.iter().zip(SIZES.iter()) {
                assert_eq!(icon.width, size);
                assert_eq!(icon.height, size);
                assert_eq!(icon.data.len(), (size * size * 4) as usize);
            }
        }
    }

    #[test]
    fn shield_is_opaque_in_the_middle_and_clear_at_the_corners() {
        let icon = &CONNECTED[0];
        let s = icon.width as usize;
        assert_eq!(alpha_at(icon, s / 2, s / 2), 255);
        for corner in [(0, 0), (s - 1, 0), (0, s - 1), (s - 1, s - 1)] {
            assert_eq!(alpha_at(icon, corner.0, corner.1), 0);
        }
    }

    #[test]
    fn shield_tapers_to_a_point() {
        // The silhouette must be narrower near the tip than at the waist.
        let width_at = |y: f32| {
            (0..1000)
                .filter(|i| in_shield(*i as f32 / 1000.0, y))
                .count()
        };
        assert!(width_at(0.90) < width_at(0.50));
        assert!(width_at(0.50) > width_at(0.08));
    }

    #[test]
    fn glyphs_stay_inside_the_shield() {
        // Clipping is by construction, but the glyph should also fit on its own
        // — otherwise strokes would end in flat cuts against the silhouette.
        for glyph in [Glyph::Check, Glyph::Cross, Glyph::Dots] {
            for i in 0..=400 {
                for j in 0..=400 {
                    let (x, y) = (i as f32 / 400.0, j as f32 / 400.0);
                    if in_glyph(&glyph, x, y) {
                        assert!(in_shield(x, y), "glyph escapes the shield at {x},{y}");
                    }
                }
            }
        }
    }
}
