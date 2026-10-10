//! Rasterizer + scene renderer. Everything is drawn at 2x resolution into an
//! RGB buffer and box-downsampled afterwards; coordinates in this file are
//! logical 1280x720 pixels unless noted.

use crate::bf::*;
use crate::fonts::*;

pub const W: i32 = 1280;
pub const H: i32 = 720;
pub const S: f32 = 2.0;

pub type C = [u8; 3];
pub const BG_TOP: C = [18, 22, 31];
pub const BG_BOT: C = [12, 15, 22];
pub const CARD: C = [26, 32, 45];
pub const CARD_BRD: C = [48, 58, 80];
pub const TEXT: C = [230, 237, 243];
pub const DIM: C = [128, 140, 160];
pub const FAINT: C = [74, 84, 104];
pub const ACCENT: C = [92, 200, 255];
pub const AMBER: C = [255, 178, 80];
pub const GREEN: C = [126, 231, 135];
pub const PURPLE: C = [196, 148, 236];
pub const RED: C = [255, 128, 120];
pub const WHITE: C = [255, 255, 255];
pub const CREAM: C = [255, 240, 214];
pub const DARK: C = [20, 24, 34];

pub fn blend(a: C, b: C, t: f32) -> C {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
    ]
}

pub fn ease_in_out(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Canvas {
    pub w: i32, // 2x pixels
    pub h: i32,
    pub px: Vec<u8>,
    clip: Option<(i32, i32, i32, i32)>,
}

impl Canvas {
    pub fn new(w: i32, h: i32, bg: C) -> Canvas {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..w * h {
            px.extend_from_slice(&bg);
        }
        Canvas { w, h, px, clip: None }
    }

    pub fn set_clip(&mut self, r: Option<(i32, i32, i32, i32)>) {
        self.clip = r;
    }

    pub fn blend(&mut self, x: i32, y: i32, c: C, cov: f32) {
        if cov <= 0.0 {
            return;
        }
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.clip {
            if x < x0 || y < y0 || x >= x1 || y >= y1 {
                return;
            }
        }
        let i = ((y * self.w + x) * 3) as usize;
        let a = cov.min(1.0);
        for k in 0..3 {
            let d = self.px[i + k] as f32;
            self.px[i + k] = (d + (c[k] as f32 - d) * a).round() as u8;
        }
    }

    fn rr_sdf(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
        let hx = (x1 - x0) * 0.5;
        let hy = (y1 - y0) * 0.5;
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        let qx = (px - cx).abs() - (hx - r);
        let qy = (py - cy).abs() - (hy - r);
        let ax = qx.max(0.0);
        let ay = qy.max(0.0);
        let outside = (ax * ax + ay * ay).sqrt();
        let inside = qx.max(qy).min(0.0);
        outside + inside - r
    }

    /// fill and/or stroke an axis-aligned rounded rect (logical coords)
    pub fn round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32,
                      fill: Option<C>, stroke: Option<(C, f32)>) {
        let (sx0, sy0) = (x0 * S, y0 * S);
        let (sx1, sy1) = (x1 * S, y1 * S);
        let sr = r * S;
        let half = stroke.as_ref().map_or(0.0, |s| s.1 * S * 0.5);
        let m = 1.5 + half;
        let ix0 = (sx0 - m).floor() as i32;
        let iy0 = (sy0 - m).floor() as i32;
        let ix1 = (sx1 + m).ceil() as i32;
        let iy1 = (sy1 + m).ceil() as i32;
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let d = Self::rr_sdf(px, py, sx0, sy0, sx1, sy1, sr);
                if let Some(c) = fill {
                    self.blend(x, y, c, (0.5 - d).clamp(0.0, 1.0));
                }
                if let Some((c, wdt)) = stroke {
                    let hw = wdt * S * 0.5;
                    self.blend(x, y, c, (hw + 0.5 - d.abs()).clamp(0.0, 1.0));
                }
            }
        }
    }

    fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
        let ex = bx - ax;
        let ey = by - ay;
        let l2 = ex * ex + ey * ey;
        let t = if l2 == 0.0 { 0.0 } else { (((px - ax) * ex + (py - ay) * ey) / l2).clamp(0.0, 1.0) };
        let dx = px - (ax + ex * t);
        let dy = py - (ay + ey * t);
        (dx * dx + dy * dy).sqrt()
    }

    pub fn line(&mut self, ax: f32, ay: f32, bx: f32, by: f32, w: f32, c: C) {
        let (ax, ay, bx, by) = (ax * S, ay * S, bx * S, by * S);
        let hw = w * S * 0.5;
        let x0 = (ax.min(bx) - hw - 1.5).floor() as i32;
        let x1 = (ax.max(bx) + hw + 1.5).ceil() as i32;
        let y0 = (ay.min(by) - hw - 1.5).floor() as i32;
        let y1 = (ay.max(by) + hw + 1.5).ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = Self::seg_dist(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by);
                self.blend(x, y, c, (hw + 0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], w: f32, c: C) {
        for pair in pts.windows(2) {
            self.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1, w, c);
        }
    }

    fn tri_sdf(px: f32, py: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
        let dot = |ux: f32, uy: f32, vx: f32, vy: f32| ux * vx + uy * vy;
        let e0 = (b.0 - a.0, b.1 - a.1);
        let e1 = (c.0 - b.0, c.1 - b.1);
        let e2 = (a.0 - c.0, a.1 - c.1);
        let v0 = (px - a.0, py - a.1);
        let v1 = (px - b.0, py - b.1);
        let v2 = (px - c.0, py - c.1);
        let cl = |e: (f32, f32), p: (f32, f32)| {
            let l = dot(e.0, e.1, e.0, e.1);
            let t = (dot(p.0, p.1, e.0, e.1) / l).clamp(0.0, 1.0);
            (p.0 - e.0 * t, p.1 - e.1 * t)
        };
        let pq0 = cl(e0, v0);
        let pq1 = cl(e1, v1);
        let pq2 = cl(e2, v2);
        let s = (e0.0 * e2.1 - e2.0 * e0.1).signum();
        let d0 = dot(pq0.0, pq0.1, pq0.0, pq0.1).min(s * (v0.0 * e0.1 - v0.1 * e0.0));
        let d1 = dot(pq1.0, pq1.1, pq1.0, pq1.1).min(s * (v1.0 * e1.1 - v1.1 * e1.0));
        let d2 = dot(pq2.0, pq2.1, pq2.0, pq2.1).min(s * (v2.0 * e2.1 - v2.1 * e2.0));
        let d = d0.min(d1).min(d2);
        if d > 0.0 { -d.sqrt() } else { (-d).sqrt() }
    }

    pub fn triangle(&mut self, a: (f32, f32), b: (f32, f32), c: (f32, f32), col: C) {
        let sc = |p: (f32, f32)| (p.0 * S, p.1 * S);
        let (a, b, c) = (sc(a), sc(b), sc(c));
        let x0 = (a.0.min(b.0).min(c.0) - 1.5).floor() as i32;
        let x1 = (a.0.max(b.0).max(c.0) + 1.5).ceil() as i32;
        let y0 = (a.1.min(b.1).min(c.1) - 1.5).floor() as i32;
        let y1 = (a.1.max(b.1).max(c.1) + 1.5).ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = Self::tri_sdf(x as f32 + 0.5, y as f32 + 0.5, a, b, c);
                self.blend(x, y, col, (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// anchor: two chars, horizontal l/m/r and vertical m/t (like PIL)
    pub fn text(&mut self, gc: &mut GlyphCache, x: f32, y: f32, s: &str, fk: u8, px: i32,
                col: C, anchor: &str, spacing: f32) {
        let wdt = gc.width(fk, px, s, spacing);
        let (asc, desc) = gc.metrics(fk, px);
        let sx = match anchor.chars().next() {
            Some('m') => x - wdt / 2.0,
            Some('r') => x - wdt,
            _ => x,
        };
        let baseline = match anchor.chars().nth(1) {
            Some('m') => y - (asc + desc) / 2.0,
            Some('t') => y - asc,
            _ => y,
        };
        let mut pen = sx * S;
        let by = baseline * S;
        for ch in s.chars() {
            let g = gc.get(fk, px, ch);
            let gx = (pen + g.ox as f32).round() as i32;
            let gy = (by + g.oy as f32).round() as i32;
            for ry in 0..g.h {
                for rx in 0..g.w {
                    let v = g.cov[(ry * g.w + rx) as usize] as f32 / 255.0;
                    self.blend(gx + rx, gy + ry, col, v);
                }
            }
            pen += (g.adv + spacing) * S;
        }
    }

    pub fn downsample(&self, ow: i32, oh: i32) -> Vec<u8> {
        let f = (self.w / ow) as usize;
        let mut out = vec![0u8; (ow * oh * 3) as usize];
        for y in 0..oh as usize {
            for x in 0..ow as usize {
                let mut acc = [0u32; 3];
                for dy in 0..f {
                    let row = ((y * f + dy) * self.w as usize + x * f) * 3;
                    for dx in 0..f {
                        let i = row + dx * 3;
                        acc[0] += self.px[i] as u32;
                        acc[1] += self.px[i + 1] as u32;
                        acc[2] += self.px[i + 2] as u32;
                    }
                }
                let n = (f * f) as u32;
                let o = (y * ow as usize + x) * 3;
                out[o] = (acc[0] / n) as u8;
                out[o + 1] = (acc[1] / n) as u8;
                out[o + 2] = (acc[2] / n) as u8;
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

pub struct Layout {
    pub code_rect: (f32, f32, f32, f32),
    pub code_px: i32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cell_gap: f32,
    pub cell_x0: f32,
    pub cell_y: f32,
    pub ptr_y: f32,
    pub out_rect: (f32, f32, f32, f32),
    pub in_rect: Option<(f32, f32, f32, f32)>,
    pub out_cols: i32,
    pub out_rows: i32,
    pub out_text_x: f32,
    pub out_text_y: f32,
    pub out_cw: f32,
    pub in_y: f32,
}

fn wrap_source(chars: &mut [SrcChar], cpl: i32) -> Vec<Vec<usize>> {
    // tokenize: nl / cmd-run / ws-run / word
    let mut tokens: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut kind = ' ';
    for (i, sc) in chars.iter().enumerate() {
        let k = if sc.ch == '\n' { 'n' } else if sc.cmd >= 0 { 'c' } else if sc.ch == ' ' { 'w' } else { 'd' };
        if k != kind || k == 'n' {
            if !cur.is_empty() {
                tokens.push(cur);
            }
            cur = Vec::new();
            kind = k;
        }
        cur.push(i);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }

    let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
    let mut col = 0i32;
    let mut emit = |lines: &mut Vec<Vec<usize>>, col: &mut i32, tok: &[usize], chars: &mut [SrcChar]| {
        for &i in tok {
            if *col >= cpl {
                lines.push(Vec::new());
                *col = 0;
            }
            chars[i].col = *col;
            chars[i].line = (lines.len() - 1) as i32;
            lines.last_mut().unwrap().push(i);
            *col += 1;
        }
    };
    for tok in tokens {
        if chars[tok[0]].ch == '\n' {
            lines.push(Vec::new());
            col = 0;
            continue;
        }
        if col > 0 && col + tok.len() as i32 > cpl {
            lines.push(Vec::new());
            col = 0;
        }
        emit(&mut lines, &mut col, &tok, chars);
    }
    lines
}

pub fn compute_layout(chars: &mut [SrcChar], gc: &mut GlyphCache, ncells: usize,
                      has_comma: bool) -> Layout {
    let m = 36.0f32;
    let code_y0 = 54.0f32;
    let gap = 16.0f32;
    let (ptr_h, cell_h, idx_h) = (22.0f32, 92.0f32, 16.0f32);
    let tape_h = ptr_h + cell_h + idx_h;
    let bot_h = 176.0f32;
    let bot_y = H as f32 - m - bot_h;
    let code_max_h = (bot_y - gap) - (code_y0 + gap) - tape_h;

    let card_w = W as f32 - 2.0 * m;
    let pad = 20.0f32;

    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut code_px = 16i32;
    let mut line_h = 0.0f32;
    for size in [16i32, 15, 14, 13, 12, 11] {
        code_px = size;
        let cw = gc.width(MONO, size * 2, "M", 0.0) / S;
        let cpl = 8.max(((card_w - 2.0 * pad) / cw) as i32);
        lines = wrap_source(chars, cpl);
        line_h = size as f32 + 10.0;
        if lines.len() as f32 * line_h + 2.0 * pad <= code_max_h {
            break;
        }
    }
    if lines.len() as f32 * line_h + 2.0 * pad > code_max_h {
        let max_lines = 1.max(((code_max_h - 2.0 * pad) / line_h) as i32);
        if (lines.len() as i32) > max_lines {
            lines.truncate(max_lines as usize);
            eprintln!("warning: source too long to display fully; trailing lines omitted");
        }
    }

    let code_h = 72.0f32.max(lines.len() as f32 * line_h + 2.0 * pad);
    let code_rect = (m, code_y0, m + card_w, code_y0 + code_h);

    let cw = gc.width(MONO, code_px * 2, "M", 0.0) / S;
    let x0 = m + pad;
    let y0 = code_y0 + pad + (code_h - 2.0 * pad - lines.len() as f32 * line_h) / 2.0;
    for (li, line) in lines.iter().enumerate() {
        for &i in line {
            chars[i].x = x0 + (chars[i].col as f32 + 0.5) * cw;
            chars[i].y = y0 + li as f32 * line_h + line_h / 2.0;
        }
    }

    let top = code_y0 + code_h + gap;
    let slack = 0.0f32.max((bot_y - gap) - top - tape_h);
    let tape_y = top + slack / 2.0;
    let ptr_y = tape_y + ptr_h;
    let avail = W as f32 - 2.0 * m;
    let cell_w = 88.0f32.min((avail - (ncells as f32 - 1.0) * 12.0) / ncells as f32);
    let cell_gap = if ncells <= 12 { 12.0 } else { 6.0 };
    let total = ncells as f32 * cell_w + (ncells as f32 - 1.0) * cell_gap;
    let cell_x0 = (W as f32 - total) / 2.0;
    let cell_y = tape_y + ptr_h;

    let out_cw = gc.width(MONO, 36, "M", 0.0) / S;
    let out_lh = 26.0f32;
    let (out_rect, in_rect) = if has_comma {
        let out_w = card_w * 0.62;
        (
            (m, bot_y, m + out_w, bot_y + bot_h),
            Some((m + out_w + gap, bot_y, m + card_w, bot_y + bot_h)),
        )
    } else {
        ((m, bot_y, m + card_w, bot_y + bot_h), None)
    };
    let out_cols = 4.max(((out_rect.2 - out_rect.0 - 2.0 * pad) / out_cw) as i32);
    let out_rows = 2.max(((out_rect.3 - out_rect.1 - 52.0) / out_lh) as i32);

    Layout {
        code_rect,
        code_px,
        cell_w,
        cell_h,
        cell_gap,
        cell_x0,
        cell_y,
        ptr_y,
        out_rect,
        in_rect,
        out_cols,
        out_rows,
        out_text_x: out_rect.0 + pad,
        out_text_y: out_rect.1 + 40.0,
        out_cw: out_cw,
        in_y: bot_y + 44.0,
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct Renderer {
    pub gc: GlyphCache,
    pub l: Layout,
    pub t: Trace,
    pub cmds: Vec<char>,
    pub cmd_src: Vec<usize>,
    pub pairs: Vec<usize>,
    pub ncells: usize,
    pub name: String,
    pub data: Vec<u8>,
    pub chars: Vec<SrcChar>,
    pub bg: Canvas,
}

impl Renderer {
    fn cell_cx(&self, i: f32) -> f32 {
        self.l.cell_x0 + i * (self.l.cell_w + self.l.cell_gap) + self.l.cell_w / 2.0
    }

    fn state(&self, idx: usize) -> (Vec<u8>, usize, usize, usize) {
        if idx >= self.t.steps.len() {
            return (self.t.final_tape.clone(), self.t.final_ptr, self.t.final_out.len(), self.t.final_inpos);
        }
        let st = &self.t.steps[idx];
        (st.tape.clone(), st.ptr, st.outlen, st.inpos)
    }

    pub fn build_background(mut self) -> Renderer {
        let l = self.l.clone_layout();
        let mut cv = Canvas::new(W * 2, H * 2, BG_TOP);
        // gradient
        for y in 0..cv.h {
            let c = blend(BG_TOP, BG_BOT, y as f32 / cv.h as f32);
            for x in 0..cv.w {
                let i = ((y * cv.w + x) * 3) as usize;
                cv.px[i] = c[0];
                cv.px[i + 1] = c[1];
                cv.px[i + 2] = c[2];
            }
        }
        // title
        cv.text(&mut self.gc, 36.0, 30.0, &self.name, SANS_BOLD, 30, TEXT, "lm", 0.0);
        let nw = self.gc.width(SANS_BOLD, 30, &self.name, 0.0) / 2.0;
        cv.text(&mut self.gc, 36.0 + nw + 12.0, 30.0, "·  brainfuck, step by step", SANS, 26, DIM, "lm", 0.0);
        // source card
        let r = l.code_rect;
        cv.round_rect(r.0, r.1, r.2, r.3, 14.0, Some(CARD), Some((CARD_BRD, 1.0)));
        for sc in self.chars.iter() {
            if sc.ch == '\n' {
                continue;
            }
            let col = if sc.cmd >= 0 {
                if self.cmds[sc.cmd as usize] == '[' || self.cmds[sc.cmd as usize] == ']' { AMBER } else { TEXT }
            } else {
                FAINT
            };
            cv.text(&mut self.gc, sc.x, sc.y, &sc.ch.to_string(), MONO, l.code_px * 2, col, "mm", 0.0);
        }
        // pointer rail
        let rx0 = self.l.cell_x0 - 20.0;
        let rx1 = self.cell_cx(self.ncells as f32 - 1.0) + self.l.cell_w / 2.0 + 20.0;
        cv.line(rx0, self.l.ptr_y, rx1, self.l.ptr_y, 1.0, FAINT);
        // cells + indices
        for i in 0..self.ncells {
            let cx = self.cell_cx(i as f32);
            cv.round_rect(cx - self.l.cell_w / 2.0, self.l.cell_y,
                          cx + self.l.cell_w / 2.0, self.l.cell_y + self.l.cell_h,
                          12.0, Some(CARD), Some((CARD_BRD, 1.0)));
            cv.text(&mut self.gc, cx, self.l.cell_y + self.l.cell_h + 8.0, &i.to_string(),
                    MONO, 22, FAINT, "mt", 0.0);
        }
        // bottom panels
        let r = self.l.out_rect;
        cv.round_rect(r.0, r.1, r.2, r.3, 14.0, Some(CARD), Some((CARD_BRD, 1.0)));
        cv.text(&mut self.gc, r.0 + 20.0, r.1 + 22.0, "OUTPUT", SANS_BOLD, 22, DIM, "lm", 2.0);
        if let Some(ir) = self.l.in_rect {
            cv.round_rect(ir.0, ir.1, ir.2, ir.3, 14.0, Some(CARD), Some((CARD_BRD, 1.0)));
            cv.text(&mut self.gc, ir.0 + 20.0, ir.1 + 22.0, "INPUT", SANS_BOLD, 22, DIM, "lm", 2.0);
            let sub = if !self.data.is_empty() {
                format!("{} byte{} on stdin", self.data.len(), if self.data.len() != 1 { "s" } else { "" })
            } else {
                "stdin empty — ',' reads EOF".into()
            };
            cv.text(&mut self.gc, ir.2 - 20.0, ir.1 + 22.0, &sub, SANS, 22, FAINT, "rm", 0.0);
        }
        self.bg = cv;
        self
    }
}

impl Layout {
    fn clone_layout(&self) -> Layout {
        Layout {
            code_rect: self.code_rect,
            code_px: self.code_px,
            cell_w: self.cell_w,
            cell_h: self.cell_h,
            cell_gap: self.cell_gap,
            cell_x0: self.cell_x0,
            cell_y: self.cell_y,
            ptr_y: self.ptr_y,
            out_rect: self.out_rect,
            in_rect: self.in_rect,
            out_cols: self.out_cols,
            out_rows: self.out_rows,
            out_text_x: self.out_text_x,
            out_text_y: self.out_text_y,
            in_y: self.in_y,
        }
    }
}
