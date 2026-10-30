//! Per-frame dynamic drawing, ported 1:1 from the reference design.

use crate::bf::*;
use crate::fonts::*;
use crate::render::*;

const CMD_DESCR: &[(char, &str)] = &[
    ('+', "increment cell"),
    ('-', "decrement cell"),
    ('>', "move pointer right"),
    ('<', "move pointer left"),
    ('.', "write byte to output"),
    (',', "read byte from input"),
    ('[', "loop start"),
    (']', "loop end"),
];

fn descr(c: char) -> &'static str {
    for &(k, v) in CMD_DESCR {
        if k == c {
            return v;
        }
    }
    ""
}

impl Renderer {
    fn draw_status(&self, cv: &mut Canvas, si: i32) {
        let n = self.t.steps.len();
        let msg = if si < 0 {
            format!("ready — {n} command{} · {ncells} cells",
                    if n != 1 { "s" } else { "" }, ncells = self.ncells)
        } else if si as usize >= n {
            let mut m = format!("done — {n} command{} executed", if n != 1 { "s" } else { "" });
            if !self.t.final_out.is_empty() {
                m.push_str(&format!(" · {} byte{} out", self.t.final_out.len(),
                                    if self.t.final_out.len() != 1 { "s" } else { "" }));
            }
            m
        } else {
            let st = &self.t.steps[si as usize];
            let mut extra = String::new();
            match st.c {
                '+' | '-' => {
                    let old = st.tape[st.ptr];
                    let new = if st.c == '+' { old.wrapping_add(1) } else { old.wrapping_sub(1) };
                    extra = format!(" · cell {}: {old} → {new}", st.ptr);
                }
                '>' | '<' => {
                    let np = (st.ptr as i32 + if st.c == '>' { 1 } else { -1 }) as usize;
                    extra = format!(" · ptr {} → {np}", st.ptr);
                }
                '.' => {
                    let b = st.tape[st.ptr];
                    extra = format!(" · '{}' ({b})", byte_glyph(b));
                }
                ',' => {
                    extra = if st.in_byte >= 0 {
                        format!(" · '{}' ({})", byte_glyph(st.in_byte as u8), st.in_byte)
                    } else {
                        " · EOF → 0".into()
                    };
                }
                '[' => {
                    extra = if st.jump == J_SKIP {
                        " · cell is 0, jump past loop".into()
                    } else {
                        " · cell ≠ 0, enter loop".into()
                    };
                }
                ']' => {
                    extra = if st.jump == J_REPEAT {
                        format!(" · cell ≠ 0, back to cmd #{}", self.pairs[st.pc] + 1)
                    } else {
                        " · cell is 0, exit loop".into()
                    };
                }
                _ => {}
            }
            format!("step {}/{} · {}  {}{extra}", si + 1, n, st.c, descr(st.c))
        };
        cv.text(&self.gc, W as f32 - 36.0, 30.0, &msg, MONO, 26, DIM, "rm", 0.0);
    }

    fn code_highlight(&self, cv: &mut Canvas, idx: usize, t: f32) {
        let st = &self.t.steps[idx];
        let sc = self.chars[self.cmd_src[st.pc]].clone();
        let cw = self.gc.width(MONO, self.l.code_px * 2, "M", 0.0) / 2.0;
        let (hw, hh) = (cw / 2.0 + 3.0, 11.5);
        let pill = blend(CARD, AMBER, (90.0 + 40.0 * (std::f32::consts::PI * t).sin()) / 255.0);
        cv.round_rect(sc.x - hw, sc.y - hh, sc.x + hw, sc.y + hh, 6.0, Some(pill), None);
        cv.text(&self.gc, sc.x, sc.y, &sc.ch.to_string(), MONO, self.l.code_px * 2, CREAM, "mm", 0.0);

        if st.c == '[' || st.c == ']' {
            let other = self.chars[self.cmd_src[self.pairs[st.pc]]].clone();
            let env = (std::f32::consts::PI * t).sin();
            if sc.line == other.line {
                let (ax, ay) = (sc.x, sc.y + hh + 2.0);
                let (bx, by) = (other.x, other.y + hh + 2.0);
                let mx = (ax + bx) / 2.0;
                let depth = (8.0 + (bx - ax).abs() * 0.18).min(30.0);
                let mut pts = Vec::new();
                for i in 0..=24 {
                    let u = i as f32 / 24.0;
                    let x = (1.0 - u) * (1.0 - u) * ax + 2.0 * (1.0 - u) * u * mx + u * u * bx;
                    let y = (1.0 - u) * (1.0 - u) * ay + 2.0 * (1.0 - u) * u * (ay + depth) + u * u * by;
                    pts.push((x, y));
                }
                cv.polyline(&pts, 2.0, blend(CARD, AMBER, 0.6 * env));
            }
            for q in [&sc, &other] {
                cv.round_rect(q.x - hw, q.y - hh, q.x + hw, q.y + hh, 6.0, None,
                              Some((blend(CARD, AMBER, 0.65 * env), 2.0)));
            }
        }
    }

    fn pointer(&self, cv: &mut Canvas, idx: usize, t: f32, active: bool) {
        let (_, ptr0, _, _) = self.state(idx);
        let x = if active && matches!(self.t.steps[idx].c, '<' | '>') {
            let (_, ptr1, _, _) = self.state(idx + 1);
            lerp(self.cell_cx(ptr0 as f32), self.cell_cx(ptr1 as f32), ease_in_out(t))
        } else {
            self.cell_cx(ptr0 as f32)
        };
        let y = self.l.ptr_y;
        cv.triangle((x - 9.0, y - 13.0), (x + 9.0, y - 13.0), (x, y - 1.0), ACCENT);
    }

    fn value_digits(&self, v: u8) -> [(char, C); 3] {
        let s = format!("{v:03}");
        let sig = format!("{v}").len();
        let mut out = [( '0', FAINT); 3];
        for (i, ch) in s.chars().enumerate() {
            out[i] = (ch, if i >= 3 - sig { TEXT } else { FAINT });
        }
        out
    }

    fn draw_roll(&self, cv: &mut Canvas, cx: f32, cy: f32, dw: f32, v0: u8, v1: u8, c: char, t: f32) {
        let l = &self.l;
        let clip = (
            ((cx - l.cell_w / 2.0) * S) as i32,
            (l.cell_y * S) as i32,
            ((cx + l.cell_w / 2.0) * S) as i32,
            ((l.cell_y + l.cell_h) * S) as i32,
        );
        cv.set_clip(Some(clip));
        let tt = ease_in_out(t);
        let roll_h = 30.0;
        let px = 52i32;
        let full_wrap = (c == '+' && v0 == 255) || (c == '-' && v0 == 0);
        if full_wrap {
            let (s0, s1) = (format!("{v0:03}"), format!("{v1:03}"));
            let dy = tt * roll_h * 1.6;
            let sign = if c == '+' { 1.0 } else { -1.0 };
            cv.text(&self.gc, cx, cy - sign * dy, &s0, MONO_BOLD, px, ACCENT, "mm", 0.0);
            cv.text(&self.gc, cx, cy + sign * (roll_h * 1.6 - dy), &s1, MONO_BOLD, px, WHITE, "mm", 0.0);
        } else {
            let d0 = format!("{v0:03}");
            let d1 = format!("{v1:03}");
            let final_digits = self.value_digits(v1);
            let sgn = if c == '-' { -1.0 } else { 1.0 };
            for k in 0..3 {
                let x = cx + (k as f32 - 1.0) * dw;
                let a = d0.as_bytes()[k].wrapping_sub(b'0') as u32;
                let b = d1.as_bytes()[k].wrapping_sub(b'0') as u32;
                if a == b {
                    let col = if final_digits[k].1 == TEXT { WHITE } else { FAINT };
                    cv.text(&self.gc, x, cy, &a.to_string(), MONO_BOLD, px, col, "mm", 0.0);
                    continue;
                }
                let steps = if c == ',' { ((b + 10 - a) % 10) as i32 } else { 1 };
                for j in 0..=steps {
                    let digit = if c == '-' { (a as i32 - j).rem_euclid(10) } else { (a as i32 + j) % 10 };
                    let y = cy + sgn * (j as f32 - tt * steps as f32) * roll_h;
                    let col = if j == 0 {
                        blend(WHITE, ACCENT, tt)
                    } else if j == steps {
                        blend(ACCENT, WHITE, tt)
                    } else {
                        ACCENT
                    };
                    cv.text(&self.gc, x, y, &digit.to_string(), MONO_BOLD, px, col, "mm", 0.0);
                }
            }
        }
        cv.set_clip(None);
    }

    fn draw_cells(&self, cv: &mut Canvas, idx: usize, t: f32, active: bool) {
        let (tape0, ptr0, _, _) = self.state(idx);
        let (tape1, ptr1, _, _) = self.state(idx + 1);
        let st = if active { Some(self.t.steps[idx].clone()) } else { None };
        let anim = matches!(&st, Some(s) if matches!(s.c, '+' | '-' | ','));

        let tints: Vec<(usize, f32)> = match &st {
            Some(s) if s.c == '<' || s.c == '>' => {
                let e = ease_in_out(t);
                vec![(ptr0, 1.0 - e), (ptr1, e)]
            }
            _ => vec![(ptr0, 1.0)],
        };
        for (ci, amt) in tints {
            if amt <= 0.01 {
                continue;
            }
            let cx = self.cell_cx(ci as f32);
            let y = self.l.cell_y;
            let (w, h) = (self.l.cell_w, self.l.cell_h);
            cv.round_rect(cx - w / 2.0, y, cx + w / 2.0, y + h, 12.0,
                          Some(blend(CARD, ACCENT, 0.12 * amt)), None);
            cv.round_rect(cx - w / 2.0, y, cx + w / 2.0, y + h, 12.0, None,
                          Some((blend(CARD, ACCENT, amt.max(0.3)), 2.0)));
        }

        let dw = self.gc.width(MONO_BOLD, 52, "0", 0.0) / 2.0;
        for i in 0..self.ncells {
            let cx = self.cell_cx(i as f32);
            let cy = self.l.cell_y + self.l.cell_h / 2.0;
            if anim && i == ptr0 {
                if let Some(s) = &st {
                    self.draw_roll(cv, cx, cy, dw, tape0[i], tape1[i], s.c, t);
                }
                continue;
            }
            let digits = self.value_digits(tape0[i]);
            let hot = i == ptr0;
            for (k, (ch, col)) in digits.iter().enumerate() {
                let col = if hot && *col == TEXT { WHITE } else { *col };
                cv.text(&self.gc, cx + (k as f32 - 1.0) * dw, cy, &ch.to_string(),
                        MONO_BOLD, 52, col, "mm", 0.0);
            }
        }
    }

    fn draw_output(&self, cv: &mut Canvas, idx: usize, t: f32, active: bool) {
        let l = &self.l;
        let (_, _, outlen, _) = self.state(idx);
        let st = if active { Some(self.t.steps[idx].clone()) } else { None };
        let typing = matches!(&st, Some(s) if s.c == '.');

        // (label, color, newline)
        let mut glyphs: Vec<(String, C, bool)> = Vec::new();
        for &b in &self.t.final_out[..outlen] {
            if b == 0x0A {
                glyphs.push(("\\n".into(), DIM, true));
            } else if (32..127).contains(&b) {
                glyphs.push(((b as char).to_string(), GREEN, false));
            } else {
                glyphs.push((byte_glyph(b), AMBER, false));
            }
        }
        let mut new_glyph = None;
        if typing {
            if let Some(s) = &st {
                let b = s.tape[s.ptr];
                let appear = (t / 0.45).clamp(0.0, 1.0);
                if b == 0x0A {
                    new_glyph = Some(("\\n".into(), blend(WHITE, DIM, appear), true));
                } else {
                    let base = if (32..127).contains(&b) { GREEN } else { AMBER };
                    new_glyph = Some((byte_glyph(b), blend(WHITE, base, appear), false));
                }
            }
        }

        let mut lines: Vec<Vec<(char, C)>> = vec![Vec::new()];
        let mut all = glyphs;
        if let Some(g) = new_glyph.clone() {
            all.push(g);
        }
        for (label, col, nl) in all {
            if nl {
                for ch in label.chars() {
                    lines.last_mut().unwrap().push((ch, col));
                }
                lines.push(Vec::new());
                continue;
            }
            for ch in label.chars() {
                if lines.last().unwrap().len() >= l.out_cols as usize {
                    lines.push(Vec::new());
                }
                lines.last_mut().unwrap().push((ch, col));
            }
        }
        let vis_start = lines.len().saturating_sub(l.out_rows as usize);
        let vis = &lines[vis_start..];

        let mut cursor = (l.out_text_x, l.out_text_y);
        let mut y = l.out_text_y;
        for (li, line) in vis.iter().enumerate() {
            let mut x = l.out_text_x;
            for (ch, col) in line {
                cv.text(&self.gc, x, y, &ch.to_string(), MONO, 36, *col, "lm", 0.0);
                x += l.out_cw;
            }
            if li == vis.len() - 1 {
                cursor = (x, y);
            }
            y += 26.0;
        }
        let (mut cx, cy) = cursor;
        if new_glyph.is_some() && t < 0.45 {
            cx -= l.out_cw;
        }
        cv.round_rect(cx + 1.0, cy - 11.0, cx + l.out_cw - 1.0, cy + 11.0, 0.0,
                      Some(blend(CARD, ACCENT, 0.5)), None);

        let shown = outlen + if typing && t >= 0.45 { 1 } else { 0 };
        let cnt = format!("{shown} byte{}", if shown != 1 { "s" } else { "" });
        cv.text(&self.gc, l.out_rect.2 - 20.0, l.out_rect.1 + 22.0, &cnt, SANS, 22, FAINT, "rm", 0.0);
    }

    fn chip_w(&self, b: u8) -> f32 {
        let label = if (32..127).contains(&b) { (b as char).to_string() } else { byte_glyph(b) };
        (self.gc.width(MONO, 28, &label, 0.0) / 2.0 + 14.0).max(26.0)
    }

    fn draw_input(&self, cv: &mut Canvas, idx: usize, t: f32, active: bool) {
        let ir = match self.l.in_rect {
            Some(r) => r,
            None => return,
        };
        let (_, _, _, inpos) = self.state(idx);
        let st = if active { Some(self.t.steps[idx].clone()) } else { None };
        let consuming = matches!(&st, Some(s) if s.c == ',');

        let x0 = ir.0 + 20.0;
        let y = self.l.in_y;
        let rem = self.data[inpos..].to_vec();
        let (chip_h, gap) = (30.0f32, 8.0);
        let max_x = ir.2 - 60.0;

        let mut shift = 0.0;
        if consuming && t > 0.5 && !rem.is_empty() {
            shift = ease_out((t - 0.5) / 0.5) * (self.chip_w(rem[0]) + gap);
        }

        let mut x = x0;
        for (k, &b) in rem.iter().enumerate() {
            let label = if (32..127).contains(&b) { (b as char).to_string() } else { byte_glyph(b) };
            let cw = self.chip_w(b);
            let xx = x - if k > 0 { shift } else { 0.0 };
            if xx + cw > max_x {
                cv.text(&self.gc, x, y, &format!("+{} more", rem.len() - k), SANS, 24, FAINT, "lm", 0.0);
                break;
            }
            let (fill, outline, tcol) = if k == 0 && consuming {
                if t < 0.5 {
                    let p = (std::f32::consts::PI * t).sin();
                    (blend(PURPLE, WHITE, p * 0.8), WHITE, DARK)
                } else {
                    x += cw + gap;
                    continue;
                }
            } else if k == 0 {
                (blend(CARD, PURPLE, 0.28), PURPLE, TEXT)
            } else {
                (CARD, CARD_BRD, TEXT)
            };
            cv.round_rect(xx, y - chip_h / 2.0, xx + cw, y + chip_h / 2.0, 8.0,
                          Some(fill), Some((outline, 1.0)));
            cv.text(&self.gc, xx + cw / 2.0, y, &label, MONO_BOLD, 28, tcol, "mm", 0.0);
            x += cw + gap;
        }

        if consuming {
            if let Some(s) = &st {
                if s.in_byte < 0 {
                    cv.text(&self.gc, x0, y + 26.0, "EOF — no bytes left, cell gets 0",
                            SANS, 24, RED, "lm", 0.0);
                }
            }
        } else if idx >= self.t.steps.len() && self.t.eof_hit {
            cv.text(&self.gc, x0, y + 26.0, "EOF was reached during this run",
                    SANS, 24, FAINT, "lm", 0.0);
        } else if self.data.is_empty() {
            cv.text(&self.gc, x0, y, "(no stdin — ',' reads EOF)", SANS, 24, FAINT, "lm", 0.0);
        }
    }

    /// si: -1 intro, >= len(steps) outro, otherwise step index; t in [0,1)
    pub fn render(&self, si: i32, t: f32) -> Canvas {
        let (idx, active) = if si < 0 {
            (0usize, false)
        } else if si as usize >= self.t.steps.len() {
            (self.t.steps.len(), false)
        } else {
            (si as usize, true)
        };
        let mut cv = self.bg.clone();
        self.draw_status(&mut cv, si);
        if active {
            self.code_highlight(&mut cv, idx, t);
        }
        self.pointer(&mut cv, idx, t, active);
        self.draw_cells(&mut cv, idx, t, active);
        self.draw_output(&mut cv, idx, t, active);
        self.draw_input(&mut cv, idx, t, active);
        cv
    }
}
