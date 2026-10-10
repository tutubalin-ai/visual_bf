//! bfviz — a step-by-step Brainfuck visualizer that renders an animated GIF.
//!
//! Single-binary tool; only two pure-Rust dependencies (ab_glyph for text,
//! gif for encoding). See README.md for the design notes.

mod bf;
mod fonts;
mod render;
mod scene;

use std::borrow::Cow;
use std::io::{IsTerminal, Read};

use bf::*;
use fonts::*;
use render::*;

const HELP: &str = "\
usage: bfviz PROGRAM [-o OUT.gif] [--cells N] [--max-steps N]
                     [--max-frames N] [--frame-ms MS]

Render a step-by-step animated GIF of a Brainfuck program
(code from a file, input from stdin).

positional arguments:
  program            path to the .bf source file

options:
  -h, --help         show this help message
  -o, --output       output GIF path (default: <program>.gif)
  --cells            tape size, 4..16 (default 10)
  --max-steps        abort after this many commands (default 20000)
  --max-frames       GIF frame budget (default 900)
  --frame-ms         delay per frame in ms, >= 20 (default 50)
";

struct Args {
    program: String,
    output: Option<String>,
    cells: usize,
    max_steps: usize,
    max_frames: u32,
    frame_ms: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        program: String::new(),
        output: None,
        cells: 10,
        max_steps: DEF_MAX_STEPS,
        max_frames: DEF_MAX_FRAMES,
        frame_ms: DEF_FRAME_MS,
    };
    let mut it = std::env::args().skip(1);
    let val = |it: &mut dyn Iterator<Item = String>, name: &str, inline: Option<String>| -> Result<String, String> {
        match inline {
            Some(v) => Ok(v),
            None => it.next().ok_or_else(|| format!("option {name} needs a value")),
        }
    };
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        match name.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-o" | "--output" => a.output = Some(val(&mut it, &name, inline)?),
            "--cells" => a.cells = val(&mut it, &name, inline)?.parse().map_err(|_| "bad --cells".to_string())?,
            "--max-steps" => a.max_steps = val(&mut it, &name, inline)?.parse().map_err(|_| "bad --max-steps".to_string())?,
            "--max-frames" => a.max_frames = val(&mut it, &name, inline)?.parse().map_err(|_| "bad --max-frames".to_string())?,
            "--frame-ms" => a.frame_ms = val(&mut it, &name, inline)?.parse().map_err(|_| "bad --frame-ms".to_string())?,
            _ if name.starts_with('-') => return Err(format!("unknown option {name}")),
            _ => a.program = name,
        }
    }
    if a.program.is_empty() {
        return Err("missing PROGRAM argument (see --help)".into());
    }
    if !(4..=16).contains(&a.cells) {
        return Err("--cells must be between 4 and 16".into());
    }
    if a.frame_ms < 20 {
        return Err("--frame-ms below 20 is clamped to 100 ms by most GIF renderers; keep it >= 20".into());
    }
    Ok(a)
}

// ---------------------------------------------------------------------------
// palette / quantization
// ---------------------------------------------------------------------------

fn palette_entries() -> Vec<C> {
    let mut v: Vec<C> = Vec::new();
    let push = |v: &mut Vec<C>, c: C| {
        if !v.contains(&c) && v.len() < 256 {
            v.push(c);
        }
    };
    for i in 0..32 {
        push(&mut v, blend(BG_TOP, BG_BOT, i as f32 / 31.0));
    }
    for c in [CARD, CARD_BRD, TEXT, DIM, FAINT, ACCENT, AMBER, GREEN, PURPLE,
              RED, WHITE, CREAM, DARK] {
        push(&mut v, c);
    }
    for fg in [TEXT, DIM, FAINT, GREEN, AMBER, ACCENT, PURPLE, RED, WHITE, CREAM] {
        for i in 1..=12 {
            push(&mut v, blend(CARD, fg, i as f32 / 12.0));
        }
    }
    for (a, b, n) in [(CARD, AMBER, 12), (CARD, ACCENT, 12), (CARD, PURPLE, 8),
                      (ACCENT, WHITE, 8), (PURPLE, WHITE, 8), (WHITE, AMBER, 8),
                      (WHITE, GREEN, 8), (WHITE, DIM, 8)] {
        for i in 1..=n {
            push(&mut v, blend(a, b, i as f32 / n as f32));
        }
    }
    while v.len() < 256 {
        v.push(CARD);
    }
    v
}

fn build_lut(pal: &[C]) -> Vec<u8> {
    let mut lut = vec![0u8; 4096];
    for (idx, cell) in lut.iter_mut().enumerate() {
        let r = (((idx >> 8) & 15) * 17) as i32;
        let g = (((idx >> 4) & 15) * 17) as i32;
        let b = ((idx & 15) * 17) as i32;
        let mut best = 0usize;
        let mut bd = i64::MAX;
        for (pi, p) in pal.iter().enumerate() {
            let dr = r - p[0] as i32;
            let dg = g - p[1] as i32;
            let db = b - p[2] as i32;
            let d = (dr * dr + dg * dg + db * db) as i64;
            if d < bd {
                bd = d;
                best = pi;
            }
        }
        *cell = best as u8;
    }
    lut
}

// ---------------------------------------------------------------------------

fn run(a: &Args) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let text = std::fs::read_to_string(&a.program)
        .map_err(|e| format!("cannot read '{}': {e}", a.program))?;
    let src = load_source(&text)?;
    let pairs = match_brackets(&src.cmds, &src.chars, &src.cmd_src)?;

    let data = if std::io::stdin().is_terminal() {
        eprintln!("note: stdin is a terminal — running with empty input (redirect a file to feed ',')");
        Vec::new()
    } else {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).map_err(|e| e.to_string())?;
        buf
    };

    let trace = execute(&src.cmds, &pairs, &data, a.cells, a.max_steps)?;
    let (plan, scaled) = frames_per_step(&trace, a.max_frames);

    let fonts = FontSet::load()?;
    let gc = GlyphCache::new(fonts);
    let mut chars = src.chars;
    let layout = compute_layout(&mut chars, &gc, a.cells, src.cmds.contains(&','));

    let name = std::path::Path::new(&a.program)
        .file_name().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| a.program.clone());

    let r = Renderer {
        gc,
        l: layout,
        t: trace,
        cmds: src.cmds,
        cmd_src: src.cmd_src,
        pairs,
        ncells: a.cells,
        name,
        data,
        chars,
        bg: Canvas::new(W * 2, H * 2, BG_TOP),
    }
    .build_background();
    r.gc.warmup();

    let n = INTRO_FRAMES + plan.iter().sum::<u32>() + OUTRO_FRAMES;
    let mut frame_ms = a.frame_ms;
    if n * frame_ms > 40_000 {
        frame_ms = 30.max(40_000 / n);
        eprintln!("note: long trace, frame delay lowered to {frame_ms} ms");
    }
    let delay_cs = (frame_ms / 10).max(2) as u16;

    let pal = palette_entries();
    let lut = build_lut(&pal);
    let mut palette_bytes: Vec<u8> = Vec::with_capacity(768);
    for c in &pal {
        palette_bytes.extend_from_slice(c);
    }

    let out = match &a.output {
        Some(o) => o.clone(),
        None => {
            let stem = a.program.trim_end_matches(".bf");
            format!("{stem}.gif")
        }
    };
    let file = std::fs::File::create(&out).map_err(|e| format!("cannot write '{out}': {e}"))?;
    let mut enc = gif::Encoder::new(file, W as u16, H as u16, &palette_bytes)
        .map_err(|e| format!("gif encoder: {e}"))?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;

    /// 2x buffer -> 1x palette indices in one fused pass.
    fn quantize_down(cv: &Canvas, lut: &[u8]) -> Vec<u8> {
        let (ow, oh) = (W as usize, H as usize);
        let src = &cv.px;
        let sw3 = (cv.w as usize) * 3;
        let mut out = vec![0u8; ow * oh];
        for y in 0..oh {
            let row = y * 2 * sw3;
            for x in 0..ow {
                let i = row + x * 6;
                let rr = (src[i] as u32 + src[i + 3] as u32 + src[i + sw3] as u32 + src[i + sw3 + 3] as u32) / 4;
                let gg = (src[i + 1] as u32 + src[i + 4] as u32 + src[i + sw3 + 1] as u32 + src[i + sw3 + 4] as u32) / 4;
                let bb = (src[i + 2] as u32 + src[i + 5] as u32 + src[i + sw3 + 2] as u32 + src[i + sw3 + 5] as u32) / 4;
                let key = ((rr / 16) as usize) << 8 | ((gg / 16) as usize) << 4 | (bb / 16) as usize;
                out[y * ow + x] = lut[key];
            }
        }
        out
    }

    fn write_frame<WR: std::io::Write>(enc: &mut gif::Encoder<WR>, idx: &[u8],
                                       delay_cs: u16) -> Result<(), String> {
        let mut frame = gif::Frame::default();
        frame.width = W as u16;
        frame.height = H as u16;
        frame.delay = delay_cs;
        frame.dispose = gif::DisposalMethod::Keep;
        frame.buffer = Cow::Borrowed(idx);
        enc.write_frame(&frame).map_err(|e| e.to_string())
    }

    let mut written = 0u32;
    // intro hold (rendered once)
    let intro = quantize_down(&r.render(-1, 0.0), &lut);
    for _ in 0..INTRO_FRAMES {
        write_frame(&mut enc, &intro, delay_cs)?;
        written += 1;
    }
    // steps — frames are independent, so render them on a thread pool and
    // encode in order afterwards
    let mut seq: Vec<(i32, f32)> = Vec::new();
    for (si, nf) in plan.iter().enumerate() {
        for k in 0..*nf {
            seq.push((si as i32, (k as f32 + 0.5) / *nf as f32));
        }
    }
    let nthreads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(4);
    for chunk in seq.chunks(64) {
        let mut bufs: Vec<Vec<u8>> = vec![Vec::new(); chunk.len()];
        if nthreads > 1 {
            std::thread::scope(|sc| {
                let per = (chunk.len() + nthreads - 1) / nthreads;
                for (pb, ps) in bufs.chunks_mut(per).zip(chunk.chunks(per)) {
                    sc.spawn(move || {
                        for ((si, t), buf) in ps.iter().zip(pb.iter_mut()) {
                            *buf = quantize_down(&r.render(*si, *t), &lut);
                        }
                    });
                }
            });
        } else {
            for ((si, t), buf) in chunk.iter().zip(bufs.iter_mut()) {
                *buf = quantize_down(&r.render(*si, *t), &lut);
            }
        }
        for b in &bufs {
            write_frame(&mut enc, b, delay_cs)?;
            written += 1;
        }
    }
    // outro hold
    let outro = quantize_down(&r.render(i32::MAX, 0.0), &lut);
    for _ in 0..OUTRO_FRAMES {
        write_frame(&mut enc, &outro, delay_cs)?;
        written += 1;
    }
    drop(enc);

    let dur = written as f64 * delay_cs as f64 * 10.0 / 1000.0;
    eprintln!(
        "wrote {out} — {written} frames, {W}x{H}, ~{dur:.0}s, {} commands executed{} (rendered in {:.2}s)",
        r.t.steps.len(),
        if scaled { ", loop frames scaled down" } else { "" },
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn main() {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&a) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
