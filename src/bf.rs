//! Parsing, bracket matching and execution tracing of a Brainfuck program.

pub const BASE_FRAMES: u32 = 6;
pub const MIN_LOOP_FR: u32 = 2;
pub const INTRO_FRAMES: u32 = 14;
pub const OUTRO_FRAMES: u32 = 40;
pub const DEF_FRAME_MS: u32 = 50;
pub const DEF_MAX_STEPS: usize = 20_000;
pub const DEF_MAX_FRAMES: u32 = 900;

#[inline]
pub fn is_cmd(c: char) -> bool {
    matches!(c, '+' | '-' | '<' | '>' | '[' | ']' | ',' | '.')
}

#[derive(Clone)]
pub struct SrcChar {
    pub ch: char,
    pub cmd: i32, // index into the filtered command list, -1 if none
    pub x: f32,
    pub y: f32,
    pub line: i32,
    pub col: i32,
}

pub struct Source {
    pub chars: Vec<SrcChar>,
    pub cmds: Vec<char>,
    pub cmd_src: Vec<usize>,
}

pub fn load_source(text: &str) -> Result<Source, String> {
    let mut chars: Vec<SrcChar> = Vec::new();
    let mut cmds: Vec<char> = Vec::new();
    let mut cmd_src: Vec<usize> = Vec::new();
    for ch in text.replace('\r', "").chars() {
        if ch == '\t' {
            chars.push(SrcChar { ch: ' ', cmd: -1, x: 0.0, y: 0.0, line: 0, col: 0 });
            chars.push(SrcChar { ch: ' ', cmd: -1, x: 0.0, y: 0.0, line: 0, col: 0 });
            continue;
        }
        if is_cmd(ch) {
            cmd_src.push(chars.len());
            chars.push(SrcChar { ch, cmd: cmds.len() as i32, x: 0.0, y: 0.0, line: 0, col: 0 });
            cmds.push(ch);
        } else {
            // keeps '\n' so wrapping sees it
            chars.push(SrcChar { ch, cmd: -1, x: 0.0, y: 0.0, line: 0, col: 0 });
        }
    }
    if cmds.is_empty() {
        return Err("source contains no Brainfuck commands (+ - < > [ ] , .)".into());
    }
    Ok(Source { chars, cmds, cmd_src })
}

/// 1-based (line, column) of a source character, for error messages.
pub fn src_line_col(chars: &[SrcChar], idx: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut last_nl: isize = -1;
    for (i, c) in chars.iter().enumerate().take(idx) {
        if c.ch == '\n' {
            line += 1;
            last_nl = i as isize;
        }
    }
    (line, (idx as isize - last_nl) as usize)
}

/// pairs[i] = index of the matching bracket for command i.
pub fn match_brackets(cmds: &[char], chars: &[SrcChar], cmd_src: &[usize]) -> Result<Vec<usize>, String> {
    let mut pairs = vec![0usize; cmds.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, c) in cmds.iter().enumerate() {
        if *c == '[' {
            stack.push(i);
        } else if *c == ']' {
            let j = match stack.pop() {
                Some(j) => j,
                None => {
                    let (l, col) = src_line_col(chars, cmd_src[i]);
                    return Err(format!("unmatched ']' (line {l}, column {col})"));
                }
            };
            pairs[j] = i;
            pairs[i] = j;
        }
    }
    if let Some(&j) = stack.last() {
        let (l, col) = src_line_col(chars, cmd_src[j]);
        return Err(format!("unmatched '[' (line {l}, column {col})"));
    }
    Ok(pairs)
}

// jump kinds
pub const J_NONE: u8 = 0;
pub const J_ENTER: u8 = 1;
pub const J_SKIP: u8 = 2;
pub const J_REPEAT: u8 = 3;
pub const J_EXIT: u8 = 4;

#[derive(Clone)]
pub struct Step {
    pub pc: usize,
    pub c: char,
    pub tape: Vec<u8>,   // state *before* the command runs
    pub ptr: usize,
    pub outlen: usize,
    pub inpos: usize,
    pub it: u32,         // iteration # of the innermost loop (0 = not in loop)
    pub jump: u8,
    pub in_byte: i32,    // byte consumed by ',' ; -1 = EOF; -2 = not a ','
}

pub struct Trace {
    pub steps: Vec<Step>,
    pub final_tape: Vec<u8>,
    pub final_ptr: usize,
    pub final_out: Vec<u8>,
    pub final_inpos: usize,
    pub eof_hit: bool,
}

pub fn execute(cmds: &[char], pairs: &[usize], data: &[u8], ncells: usize, max_steps: usize) -> Result<Trace, String> {
    let mut tape = vec![0u8; ncells];
    let mut ptr = 0usize;
    let mut pc = 0usize;
    let mut inpos = 0usize;
    let mut out: Vec<u8> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut loop_stack: Vec<usize> = Vec::new();
    let mut iters: Vec<u32> = vec![1; cmds.len()];
    let mut eof_hit = false;

    while pc < cmds.len() {
        if steps.len() >= max_steps {
            return Err(format!(
                "program still running after {max_steps} commands — too long to visualise (raise --max-steps if you must)"));
        }
        let c = cmds[pc];
        let it = match loop_stack.last() {
            Some(&lp) => iters[lp],
            None => 0,
        };
        let mut st = Step { pc, c, tape: tape.clone(), ptr, outlen: out.len(), inpos, it, jump: J_NONE, in_byte: -2 };

        match c {
            '+' => tape[ptr] = tape[ptr].wrapping_add(1),
            '-' => tape[ptr] = tape[ptr].wrapping_sub(1),
            '>' => {
                ptr += 1;
                if ptr >= ncells {
                    return Err(format!(
                        "command #{} ('>') moves the pointer past cell {}; the tape only has {ncells} cells",
                        pc + 1, ncells - 1));
                }
            }
            '<' => {
                if ptr == 0 {
                    return Err(format!("command #{} ('<') moves the pointer before cell 0", pc + 1));
                }
                ptr -= 1;
            }
            '.' => out.push(tape[ptr]),
            ',' => {
                let b = if inpos < data.len() {
                    let b = data[inpos];
                    inpos += 1;
                    b as i32
                } else {
                    eof_hit = true;
                    0
                };
                st.in_byte = b;
                tape[ptr] = b as u8;
            }
            '[' => {
                if tape[ptr] == 0 {
                    st.jump = J_SKIP;
                    steps.push(st);
                    pc = pairs[pc] + 1;
                    continue;
                }
                st.jump = J_ENTER;
                loop_stack.push(pc);
            }
            ']' => {
                let open = pairs[pc];
                if tape[ptr] != 0 {
                    st.jump = J_REPEAT;
                    iters[open] += 1;
                    steps.push(st);
                    pc = open + 1;
                    continue;
                }
                st.jump = J_EXIT;
                if loop_stack.last() == Some(&open) {
                    loop_stack.pop();
                }
            }
            _ => {}
        }
        steps.push(st);
        pc += 1;
    }

    Ok(Trace { steps, final_tape: tape, final_ptr: ptr, final_out: out, final_inpos: inpos, eof_hit })
}

/// Frame count per step: first two loop iterations full speed, then faster.
pub fn frames_per_step(trace: &Trace, max_frames: u32) -> (Vec<u32>, bool) {
    let mut plan: Vec<u32> = trace
        .steps
        .iter()
        .map(|st| {
            if st.it <= 2 {
                BASE_FRAMES
            } else if st.it == 3 {
                MIN_LOOP_FR.max(BASE_FRAMES - 2)
            } else {
                MIN_LOOP_FR
            }
        })
        .collect();
    let budget = max_frames.saturating_sub(INTRO_FRAMES + OUTRO_FRAMES);
    let total: u32 = plan.iter().sum();
    let mut scaled = false;
    if total > budget && budget > 0 {
        scaled = true;
        let k = budget as f64 / total as f64;
        for f in plan.iter_mut() {
            *f = 1.max((*f as f64 * k).round() as u32);
        }
    }
    (plan, scaled)
}

/// How a byte is shown in the output stream.
pub fn byte_glyph(b: u8) -> String {
    match b {
        0x0A => "\\n".into(),
        0x0D => "\\r".into(),
        0x09 => "\\t".into(),
        32..=126 => (b as char).to_string(),
        _ => format!("\\x{b:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, data: &[u8]) -> (Vec<u8>, usize) {
        let src = load_source(text).unwrap();
        let pairs = match_brackets(&src.cmds, &src.chars, &src.cmd_src).unwrap();
        let tr = execute(&src.cmds, &pairs, data, 30, 1_000_000).unwrap();
        (tr.final_out.clone(), tr.steps.len())
    }

    #[test]
    fn hello() {
        let (out, _) = run(
            "++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++.",
            b"");
        assert_eq!(out, b"Hello World!\n");
    }

    #[test]
    fn wrapping_cells() {
        let (out, _) = run("-.", b"");
        assert_eq!(out, [255]);
        let (out, _) = run(&format!("{}.", "+".repeat(256)), b"");
        assert_eq!(out, [0]); // 256 wraps to 0
    }

    #[test]
    fn eof_is_zero() {
        let (out, _) = run(",.", b"");
        assert_eq!(out, [0]);
        let (out, _) = run(",[.,]", b"");
        assert_eq!(out, [0]);
    }

    #[test]
    fn unbalanced() {
        let src = load_source("+++[").unwrap();
        assert!(match_brackets(&src.cmds, &src.chars, &src.cmd_src).is_err());
        let src = load_source("]").unwrap();
        assert!(match_brackets(&src.cmds, &src.chars, &src.cmd_src).is_err());
    }

    #[test]
    fn comments_are_ignored() {
        let (out, _) = run("plus: ++ dot: . (prints chr(2))", b"");
        assert_eq!(out, [2]);
    }

    #[test]
    fn loop_speed_plan() {
        let src = load_source("++++++++++[>[-]++++++++++++++++++++++++++++++++++++++++++.<-]").unwrap();
        let pairs = match_brackets(&src.cmds, &src.chars, &src.cmd_src).unwrap();
        let tr = execute(&src.cmds, &pairs, b"", 10, 100_000).unwrap();
        let (plan, _) = frames_per_step(&tr, 900);
        assert_eq!(plan.len(), tr.steps.len());
        // deep loop iterations must be faster than the first ones
        assert!(plan[plan.len() / 2] <= MIN_LOOP_FR.max(2));
    }
}
