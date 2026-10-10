# visual_bf

`bfviz` turns a small [Brainfuck](https://esolangs.org/wiki/Brainfuck)
program into a 1280×720 animated GIF that shows, step by step, what the
machine is doing. It was made to illustrate BF programs in articles: clear,
calm, and complete — no step is ever skipped, long loops just play faster.

Written in Rust with exactly two pure-Rust dependencies (`ab_glyph` for
text, `gif` for encoding). Renders at 2× and box-downsamples for crisp
anti-aliased output; a typical 500-frame GIF renders in a couple of seconds.

## Quick start

```
cargo build --release

./target/release/bfviz examples/add.bf                     # no input
printf 'hi\n' | ./target/release/bfviz examples/cat.bf     # program reads stdin
```

The GIF is written next to the source file (`add.bf` → `add.gif`) unless
`-o` says otherwise.

## What you see

| area      | shows |
|-----------|-------|
| header    | file name, live status line: `step i/N · cmd · meaning · before → after` |
| source    | the whole file. Anything that is not one of `+ - < > [ ] , .` is a comment and drawn dim; the command that is executing is highlighted. `[` / `]` are linked with a glowing arc while one of them runs. |
| tape      | 10 cells (adjustable), values shown as 3‑digit odometer digits with dimmed leading zeros, pointer arrow above the active cell |
| output    | the program's output typed like on a typewriter with a block cursor; unprintable bytes shown as `\n`, `\t`, `\xHH` |
| input     | the bytes waiting on stdin as chips; the next one to be read is lit; a consumed chip flashes and the rest slide left |

### Animations

* `+` / `-` — odometer roll: the changed digit rolls up (down for `-`),
  carries included (`019 → 020` rolls two digits). The `255 ↔ 0` wrap rolls
  the whole number like one drum.
* `<` / `>` — the pointer arrow glides with an ease curve and the active‑cell
  highlight fades over to the neighbour.
* `.` — the byte is "stamped" under the cursor, which then advances.
* `,` — the front input chip flashes away and the cell spins up to the new
  byte value, digit by digit.
* `[` / `]` — both brackets get a ring and (when on the same line) an arc;
  the status line says *enter loop / jump past loop / back to cmd #k / exit
  loop* so a skipped loop is as visible as a repeated one.

### Speed

Every command normally gets 6 frames (~0.3 s at the default 50 ms/frame).
Loop iterations 3+ get 4, then 2 frames per command, so a long loop visibly
accelerates after you have understood it. A global frame budget
(`--max-frames`, default 900) scales everything down further if needed, and
if the result would run longer than ~40 s the frame delay is lowered
automatically (never below 30 ms). The GIF opens with a short hold on the
initial state and ends with a longer hold on the final state.

## Semantics (the boring but important part)

* cells are 8 bit and wrap both ways (`0 - 1 → 255`, `255 + 1 → 0`)
* the tape has a fixed size, 10 cells by default (`--cells 4..16`);
  running past either end is an error that names the offending command
* `,` at end of input stores `0` and the input panel notes the EOF
* anything that is not a BF command — whitespace, letters, `# comments`,
  `( comments )`, … — is ignored, so sources can be commented freely
* unbalanced brackets, sources without commands, and programs that do not
  finish within `--max-steps` (default 20 000 commands) are reported with
  line/column or command number instead of producing a GIF

## CLI

```
bfviz PROGRAM [-o OUT.gif] [--cells N] [--max-steps N]
              [--max-frames N] [--frame-ms MS]
```

* `PROGRAM` — Brainfuck source file (input comes from **stdin**)
* `-o` — output path (default: `<PROGRAM>.gif`)
* `--cells` — tape size, 4..16 (default 10)
* `--max-steps` — abort longer runs (default 20000)
* `--max-frames` — GIF frame budget (default 900)
* `--frame-ms` — delay per frame, ≥ 20 (default 50)

## Examples

* `examples/add.bf` — 3 + 5, printed as `8`; the smallest demo
* `examples/hello.bf` — the canonical Hello World (loops inside loops)
* `examples/stars.bf` — a loop that prints ten stars, shows the speed‑up
* `examples/cat.bf` — echoes stdin until EOF; feed it with
  `printf 'hi\n' | ./target/release/bfviz examples/cat.bf`

## Implementation notes

Frames are drawn at 2× resolution into an RGB buffer, box‑downsampled, and
quantized against a fixed 256‑colour palette built from colour ramps, so
flat areas stay flat and text keeps smooth edges across all frames. The
program is traced completely before rendering, which is what makes the
adaptive loop speed and the `step i/N` counter possible. Rendering is
deterministic: same source + same stdin → same GIF.

The CI workflow builds, runs the unit tests (interpreter semantics), renders
all examples and validates the GIFs (size, frame count, duration) on every
push.

Fonts: the first one found of DejaVu Sans Mono / Liberation Mono / Menlo /
Consolas (and DejaVu Sans etc. for labels). If none exists, `bfviz` exits
with an error instead of drawing with a bitmap font.
