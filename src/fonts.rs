//! System-font loading and cached glyph rasterization (ab_glyph).

use ab_glyph::{point, Font, FontArc, Glyph, PxScale, ScaleFont};
use std::collections::HashMap;

pub const MONO: u8 = 0;
pub const MONO_BOLD: u8 = 1;
pub const SANS: u8 = 2;
pub const SANS_BOLD: u8 = 3;

const FONT_DIRS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu",
    "/usr/share/fonts/dejavu",
    "/usr/share/fonts/truetype",
    "/usr/share/fonts/TTF",
    "/usr/share/fonts",
    "/System/Library/Fonts",
    "C:/Windows/Fonts",
];

fn try_load(names: &[(&str, usize)]) -> Result<FontArc, String> {
    for dir in FONT_DIRS {
        for (name, idx) in names {
            let p = format!("{dir}/{name}");
            if let Ok(data) = std::fs::read(&p) {
                if let Ok(f) = FontArc::try_from_vec_and_index(data, *idx) {
                    return Ok(f);
                }
            }
        }
    }
    Err(format!("no usable font found (tried {names:?} in standard dirs)"))
}

pub struct FontSet {
    pub fonts: [FontArc; 4],
}

impl FontSet {
    pub fn load() -> Result<FontSet, String> {
        Ok(FontSet {
            fonts: [
                try_load(&[
                    ("DejaVuSansMono.ttf", 0),
                    ("LiberationMono-Regular.ttf", 0),
                    ("Menlo.ttc", 0),
                    ("consolas.ttf", 0),
                ])?,
                try_load(&[
                    ("DejaVuSansMono-Bold.ttf", 0),
                    ("LiberationMono-Bold.ttf", 0),
                    ("Menlo.ttc", 1),
                    ("consolab.ttf", 0),
                ])?,
                try_load(&[
                    ("DejaVuSans.ttf", 0),
                    ("LiberationSans-Regular.ttf", 0),
                    ("Helvetica.ttc", 0),
                    ("arial.ttf", 0),
                ])?,
                try_load(&[
                    ("DejaVuSans-Bold.ttf", 0),
                    ("LiberationSans-Bold.ttf", 0),
                    ("Helvetica.ttc", 1),
                    ("arialbd.ttf", 0),
                ])?,
            ],
        })
    }
}

#[derive(Clone)]
pub struct CachedGlyph {
    pub adv: f32,
    pub ox: i32,
    pub oy: i32,
    pub w: i32,
    pub h: i32,
    pub cov: Vec<u8>,
}

pub struct GlyphCache {
    fonts: FontSet,
    map: HashMap<u64, CachedGlyph>,
}

fn key(fk: u8, px: i32, ch: char) -> u64 {
    ((fk as u64) << 44) | ((px as u64 & 0xFFFFF) << 32) | (ch as u32 as u64)
}

impl GlyphCache {
    pub fn new(fonts: FontSet) -> GlyphCache {
        GlyphCache { fonts, map: HashMap::new() }
    }

    pub fn get(&mut self, fk: u8, px: i32, ch: char) -> CachedGlyph {
        let k = key(fk, px, ch);
        if let Some(g) = self.map.get(&k) {
            return g.clone();
        }
        let font = &self.fonts.fonts[fk as usize];
        let scale = PxScale::from(px as f32);
        let sf = font.as_scaled(scale);
        let id = font.glyph_id(ch);
        let adv = sf.h_advance(id);
        let (mut ox, mut oy, mut w, mut h) = (0i32, 0i32, 0i32, 0i32);
        let mut cov: Vec<u8> = Vec::new();
        if let Some(og) = font.outline_glyph(Glyph { id, scale, position: point(0.0, 0.0) }) {
            let bb = og.px_bounds();
            ox = bb.min.x.floor() as i32;
            oy = bb.min.y.floor() as i32;
            w = (bb.max.x.ceil() as i32) - ox;
            h = (bb.max.y.ceil() as i32) - oy;
            if w > 0 && h > 0 {
                cov.resize((w * h) as usize, 0);
                og.draw(|x, y, v| {
                    let i = (y as i32 * w + x as i32) as usize;
                    if i < cov.len() {
                        cov[i] = (v * 255.0).round() as u8;
                    }
                });
            } else {
                w = 0;
                h = 0;
            }
        }
        let g = CachedGlyph { adv, ox, oy, w, h, cov };
        self.map.insert(k, g.clone());
        g
    }

    /// (ascent, descent) in px; descent is negative.
    pub fn metrics(&self, fk: u8, px: i32) -> (f32, f32) {
        let font = &self.fonts.fonts[fk as usize];
        let sf = font.as_scaled(PxScale::from(px as f32));
        (sf.ascent(), sf.descent())
    }

    pub fn width(&mut self, fk: u8, px: i32, s: &str, spacing: f32) -> f32 {
        let mut w = 0.0;
        for ch in s.chars() {
            w += self.get(fk, px, ch).adv + spacing;
        }
        w - spacing
    }
}
