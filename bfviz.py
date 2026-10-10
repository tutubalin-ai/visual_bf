#!/usr/bin/env python3
"""
bfviz — a step-by-step Brainfuck visualizer that renders an animated GIF.

Designed to illustrate *small* Brainfuck programs in articles: it shows the
source code (comments and stray characters included), a 10-cell tape with a
moving pointer, the program's output "typed" like on a typewriter, and the
bytes waiting on stdin.

Animations:
  * `+` / `-`  : odometer-style digit roll (carries included)
  * `>` / `<`  : eased pointer slide, active-cell highlight glides along
  * `.`        : typewriter stamp of the output byte
  * `,`        : the next input chip is consumed, cell spins to the new value
  * `[` / `]`  : the matching bracket pair is linked with a glowing arc

Long loops are detected while tracing: the first two iterations run at full
speed, later iterations get progressively fewer frames per command so the GIF
stays short without ever skipping program behaviour.

Semantics (classic, documented in README.md):
  * cells are 8-bit and wrap around (0 - 1 -> 255, 255 + 1 -> 0)
  * the tape has a fixed, small number of cells (default 10)
  * EOF on `,` stores 0

Usage:
    python3 bfviz.py program.bf < input.txt -o out.gif

Only dependency: Pillow.
"""
from __future__ import annotations

import argparse
import math
import os
import sys
from dataclasses import dataclass
from typing import Iterator, Optional

from PIL import Image, ImageDraw, ImageFont

# ----------------------------------------------------------------------------
# Constants & theme
# ----------------------------------------------------------------------------

W, H = 1280, 720          # final GIF size (required)
S = 2                     # supersampling factor (rendered at 2560x1440)

CMDS = set("+-<>[],.")

# Palette (dark, low-contrast cards, bright accents)
BG_TOP     = (18, 22, 31)
BG_BOT     = (12, 15, 22)
CARD       = (26, 32, 45)
CARD_BRD   = (48, 58, 80)
TEXT       = (230, 237, 243)
DIM        = (128, 140, 160)
FAINT      = (74, 84, 104)
ACCENT     = (92, 200, 255)   # cyan   — pointer / active cell / rolls
AMBER      = (255, 178, 80)   # amber  — active command / brackets
GREEN      = (126, 231, 135)  # green  — program output
PURPLE     = (196, 148, 236)  # purple — stdin chips
RED        = (255, 128, 120)

CMD_DESCR = {
    "+": "increment cell",
    "-": "decrement cell",
    ">": "move pointer right",
    "<": "move pointer left",
    ".": "write byte to output",
    ",": "read byte from input",
    "[": "loop start",
    "]": "loop end",
}

BASE_FRAMES   = 6     # frames per command at normal speed
MIN_LOOP_FR   = 2     # frames per command deep inside a repeating loop
INTRO_FRAMES  = 14    # ~0.7 s on the untouched initial state
OUTRO_FRAMES  = 40    # ~2 s on the final state
DEF_FRAME_MS  = 50    # GIF frame delay (ms); keep >= 20 for sane renderers
DEF_MAX_STEPS = 20_000
DEF_MAX_FRAMES = 900  # frame budget (900 * 50 ms = 45 s)


class BfError(Exception):
    """User-facing error (bad program, bad input, ...)."""


# ----------------------------------------------------------------------------
# Font handling (DejaVu if present, Pillow's built-in font otherwise)
# ----------------------------------------------------------------------------

_FONT_DIRS = [
    "/usr/share/fonts/truetype/dejavu",
    "/usr/share/fonts/dejavu",
    "/usr/share/fonts/truetype",
    "/usr/share/fonts/TTF",
    "/usr/share/fonts",
    "/System/Library/Fonts",
    "C:/Windows/Fonts",
]


def _find(name: str) -> Optional[str]:
    for d in _FONT_DIRS:
        p = os.path.join(d, name)
        if os.path.isfile(p):
            return p
    return None


def _ttf(names: list[str], size: int) -> ImageFont.FreeTypeFont:
    for n in names:
        p = _find(n)
        if p:
            try:
                return ImageFont.truetype(p, size * S)
            except OSError:
                pass
    try:
        return ImageFont.load_default(size * S)
    except TypeError:                      # very old Pillow
        return ImageFont.load_default()


class Fonts:
    def __init__(self) -> None:
        self._cache: dict[tuple, ImageFont.FreeTypeFont] = {}

    def mono(self, size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
        key = ("m", size, bold)
        if key not in self._cache:
            names = (["DejaVuSansMono-Bold.ttf", "LiberationMono-Bold.ttf"] if bold
                     else ["DejaVuSansMono.ttf", "LiberationMono-Regular.ttf"])
            self._cache[key] = _ttf(names, size)
        return self._cache[key]

    def sans(self, size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
        key = ("s", size, bold)
        if key not in self._cache:
            names = (["DejaVuSans-Bold.ttf"] if bold else ["DejaVuSans.ttf"])
            self._cache[key] = _ttf(names, size)
        return self._cache[key]


FONTS = Fonts()


# ----------------------------------------------------------------------------
# Parsing / source layout
# ----------------------------------------------------------------------------

@dataclass
class SrcChar:
    ch: str
    cmd: int = -1          # index into the filtered command list, -1 if none
    x: float = 0.0         # centre position (logical px)
    y: float = 0.0
    line: int = 0
    col: int = 0


def load_source(path: str) -> tuple[list[SrcChar], list[str], list[int], str]:
    """Read the program file.

    Returns (chars, cmds, cmd_src, text) where
      chars   — every displayable character of the source (comments included)
      cmds    — only the 8 BF commands
      cmd_src — cmd index -> index into chars
      text    — raw source text
    """
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
    except OSError as e:
        raise BfError(f"cannot read '{path}': {e}")

    chars: list[SrcChar] = []
    cmds: list[str] = []
    cmd_src: list[int] = []
    for ch in text.replace("\r", ""):
        if ch == "\t":
            ch = "  "
        if ch in CMDS:
            cmd_src.append(len(chars))
            chars.append(SrcChar(ch, cmd=len(cmds)))
            cmds.append(ch)
        else:                               # keeps '\n' so wrapping sees it
            chars.append(SrcChar(ch))
    if not cmds:
        raise BfError(f"'{path}' contains no Brainfuck commands (+ - < > [ ] , .)")
    return chars, cmds, cmd_src, text


def src_line_col(chars: list[SrcChar], idx: int) -> tuple[int, int]:
    """1-based (line, column) of a source character, for error messages."""
    line = 1 + sum(1 for c in chars[:idx] if c.ch == "\n")
    last_nl = -1
    for i in range(idx):
        if chars[i].ch == "\n":
            last_nl = i
    return line, idx - last_nl


def match_brackets(cmds: list[str], chars: list[SrcChar],
                   cmd_src: list[int]) -> dict[int, int]:
    pairs: dict[int, int] = {}
    stack: list[int] = []
    for i, c in enumerate(cmds):
        if c == "[":
            stack.append(i)
        elif c == "]":
            if not stack:
                l, col = src_line_col(chars, cmd_src[i])
                raise BfError(f"unmatched ']' (line {l}, column {col})")
            j = stack.pop()
            pairs[j] = i
            pairs[i] = j
    if stack:
        l, col = src_line_col(chars, cmd_src[stack[-1]])
        raise BfError(f"unmatched '[' (line {l}, column {col})")
    return pairs


# ----------------------------------------------------------------------------
# Execution trace
# ----------------------------------------------------------------------------

@dataclass
class Step:
    pc: int
    c: str
    tape: tuple            # state *before* the command runs
    ptr: int
    outlen: int
    inpos: int
    it: int                # iteration # of the innermost loop (0 = not in loop)
    jump: Optional[str]    # 'enter' 'skip' 'repeat' 'exit' for brackets
    in_byte: Optional[int] # byte consumed by ',' ; -1 means EOF


@dataclass
class Trace:
    steps: list[Step]
    final_tape: tuple
    final_ptr: int
    final_out: bytes
    final_inpos: int
    eof_hit: bool


def execute(cmds: list[str], pairs: dict[int, int], data: bytes,
            ncells: int, max_steps: int) -> Trace:
    tape = [0] * ncells
    ptr = pc = inpos = 0
    out = bytearray()
    steps: list[Step] = []
    loop_stack: list[int] = []        # pc of every entered '['
    iters: dict[int, int] = {}        # loop pc -> current iteration number
    eof_hit = False

    while pc < len(cmds):
        if len(steps) >= max_steps:
            raise BfError(
                f"program still running after {max_steps} commands — "
                f"too long to visualise (raise --max-steps if you must)")
        c = cmds[pc]
        it = iters.get(loop_stack[-1], 1) if loop_stack else 0
        st = Step(pc, c, tuple(tape), ptr, len(out), inpos, it, None, None)

        if c == "+":
            tape[ptr] = (tape[ptr] + 1) & 0xFF
        elif c == "-":
            tape[ptr] = (tape[ptr] - 1) & 0xFF
        elif c == ">":
            ptr += 1
            if ptr >= ncells:
                raise BfError(f"command #{pc + 1} ('>') moves the pointer past "
                              f"cell {ncells - 1}; the tape only has {ncells} cells")
        elif c == "<":
            ptr -= 1
            if ptr < 0:
                raise BfError(f"command #{pc + 1} ('<') moves the pointer "
                              f"before cell 0")
        elif c == ".":
            out.append(tape[ptr])
        elif c == ",":
            if inpos < len(data):
                b = data[inpos]
                inpos += 1
            else:
                b = 0
                eof_hit = True
            st.in_byte = b
            tape[ptr] = b
        elif c == "[":
            if tape[ptr] == 0:
                st.jump = "skip"
                steps.append(st)
                pc = pairs[pc] + 1
                continue
            st.jump = "enter"
            loop_stack.append(pc)
            iters.setdefault(pc, 1)
        elif c == "]":
            open_pc = pairs[pc]
            if tape[ptr] != 0:
                st.jump = "repeat"
                iters[open_pc] += 1
                steps.append(st)
                pc = open_pc + 1
                continue
            st.jump = "exit"
            if loop_stack and loop_stack[-1] == open_pc:
                loop_stack.pop()

        steps.append(st)
        pc += 1

    return Trace(steps, tuple(tape), ptr, bytes(out), inpos, eof_hit)


def frames_per_step(trace: Trace, max_frames: int) -> tuple[list[int], bool]:
    """Assign an animation frame count to every step.

    First two iterations of a loop get the full budget, later ones speed up.
    The whole trace is then scaled down if it would exceed the frame budget.
    """
    plan: list[int] = []
    for st in trace.steps:
        if st.it <= 2:
            plan.append(BASE_FRAMES)
        elif st.it == 3:
            plan.append(max(MIN_LOOP_FR, BASE_FRAMES - 2))
        else:
            plan.append(MIN_LOOP_FR)
    budget = max_frames - INTRO_FRAMES - OUTRO_FRAMES
    total = sum(plan)
    scaled = False
    if total > budget > 0:
        scaled = True
        k = budget / total
        plan = [max(1, int(round(f * k))) for f in plan]
    return plan, scaled


# ----------------------------------------------------------------------------
# Small math helpers
# ----------------------------------------------------------------------------

def ease_in_out(t: float) -> float:
    return t * t * (3 - 2 * t)


def ease_out(t: float) -> float:
    return 1 - (1 - t) * (1 - t)


def clamp01(t: float) -> float:
    return max(0.0, min(1.0, t))


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def blend(c1: tuple, c2: tuple, t: float) -> tuple:
    return tuple(round(lerp(x, y, t)) for x, y in zip(c1, c2))


def byte_glyph(b: int) -> str:
    """How a byte is shown in the output stream."""
    if b == 0x0A:
        return "\\n"
    if b == 0x0D:
        return "\\r"
    if b == 0x09:
        return "\\t"
    if 32 <= b < 127:
        return chr(b)
    return f"\\x{b:02X}"


# ----------------------------------------------------------------------------
# Layout
# ----------------------------------------------------------------------------

@dataclass
class Layout:
    code_rect: tuple          # x0, y0, x1, y1 of the source card
    src: list[SrcChar]        # chars with .x/.y/.line/.col filled in
    code_font: ImageFont.FreeTypeFont
    cell_w: float
    cell_h: float
    cell_gap: float
    cell_x0: float            # left edge of cell 0
    cell_y: float             # top edge of the cells
    ptr_y: float              # baseline row of the pointer rail
    out_rect: tuple
    in_rect: Optional[tuple]
    out_cols: int
    out_rows: int
    out_text_x: float
    out_text_y: float
    out_font: ImageFont.FreeTypeFont
    out_cw: float
    out_lh: float
    in_y: float               # chip baseline


def wrap_source(chars: list[SrcChar], cpl: int) -> list[list[SrcChar]]:
    """Wrap the source into lines of at most cpl characters.

    Command runs wrap at the line edge (a run of '+' must stay contiguous),
    comment text wraps at word boundaries like a normal editor would.
    """
    tokens: list[list[SrcChar]] = []
    cur: list[SrcChar] = []
    kind = ""                                      # cmd / ws / word / nl
    for sc in chars:
        k = ("nl" if sc.ch == "\n" else "cmd" if sc.cmd >= 0
             else "ws" if sc.ch == " " else "word")
        if k != kind or k in ("nl",):
            if cur:
                tokens.append(cur)
            cur = []
            kind = k
        cur.append(sc)
    if cur:
        tokens.append(cur)

    lines: list[list[SrcChar]] = [[]]
    col = 0

    def emit(tok: list[SrcChar]) -> None:
        nonlocal col
        for sc in tok:
            if col >= cpl:
                lines.append([])
                col = 0
            sc.col = col
            sc.line = len(lines) - 1
            lines[-1].append(sc)
            col += 1

    for tok in tokens:
        if tok[0].ch == "\n":
            lines.append([])
            col = 0
            continue
        if col and col + len(tok) > cpl:
            lines.append([])
            col = 0
        emit(tok)                                  # long runs hard-wrap
    return lines


def compute_layout(chars: list[SrcChar], ncells: int,
                   has_comma: bool) -> Layout:
    M = 36.0
    code_y0 = 54.0
    gap = 16.0
    ptr_h, cell_h, idx_h = 22.0, 92.0, 16.0
    tape_h = ptr_h + cell_h + idx_h
    bot_h = 176.0
    bot_y = H - M - bot_h
    code_max_h = (bot_y - gap) - (code_y0 + gap) - tape_h

    card_w = W - 2 * M
    pad = 20.0

    # -- choose a code font size that fits -------------------------------
    lines = None
    code_font = None
    line_h = cpl = 0
    for size in (16, 15, 14, 13, 12, 11):
        code_font = FONTS.mono(size)
        cw = code_font.getlength("M") / S
        cpl = max(8, int((card_w - 2 * pad) // cw))
        lines = wrap_source(chars, cpl)
        line_h = size + 10
        if len(lines) * line_h + 2 * pad <= code_max_h:
            break
    assert lines is not None
    if len(lines) * line_h + 2 * pad > code_max_h:      # still too tall: cut
        max_lines = max(1, int((code_max_h - 2 * pad) // line_h))
        lines = lines[:max_lines]
        sys.stderr.write("warning: source too long to display fully; "
                         "trailing lines omitted\n")

    code_h = max(72.0, len(lines) * line_h + 2 * pad)
    code_rect = (M, code_y0, M + card_w, code_y0 + code_h)

    cw = code_font.getlength("M") / S
    x0 = M + pad
    y0 = code_y0 + pad + (code_h - 2 * pad - len(lines) * line_h) / 2
    for li, line in enumerate(lines):
        for sc in line:
            sc.x = x0 + (sc.col + 0.5) * cw
            sc.y = y0 + li * line_h + line_h / 2

    # -- tape (vertically centred in the remaining space) -----------------
    top = code_y0 + code_h + gap
    slack = max(0.0, (bot_y - gap) - top - tape_h)
    tape_y = top + slack / 2
    ptr_y = tape_y + ptr_h
    avail = W - 2 * M
    cell_w = min(88.0, (avail - (ncells - 1) * 12) / ncells)
    cell_gap = 12.0 if ncells <= 12 else 6.0
    total = ncells * cell_w + (ncells - 1) * cell_gap
    cell_x0 = (W - total) / 2
    cell_y = tape_y + ptr_h

    # -- bottom panels -----------------------------------------------------
    out_font = FONTS.mono(18)
    out_cw = out_font.getlength("M") / S
    out_lh = 26.0

    if has_comma:
        out_w = card_w * 0.62
        in_w = card_w - out_w - gap
        out_rect = (M, bot_y, M + out_w, bot_y + bot_h)
        in_rect = (M + out_w + gap, bot_y, M + card_w, bot_y + bot_h)
    else:
        out_rect = (M, bot_y, M + card_w, bot_y + bot_h)
        in_rect = None

    out_cols = max(4, int((out_rect[2] - out_rect[0] - 2 * pad) // out_cw))
    out_rows = max(2, int((out_rect[3] - out_rect[1] - 52) // out_lh))

    return Layout(code_rect, chars, code_font, cell_w, cell_h, cell_gap,
                  cell_x0, cell_y, ptr_y, out_rect, in_rect,
                  out_cols, out_rows,
                  out_rect[0] + pad, out_rect[1] + 40.0,
                  out_font, out_cw, out_lh,
                  bot_y + 44.0)


# ----------------------------------------------------------------------------
# Renderer
# ----------------------------------------------------------------------------

class Renderer:
    def __init__(self, layout: Layout, trace: Trace, cmds: list[str],
                 cmd_src: list[int], pairs: dict[int, int],
                 ncells: int, prog_name: str, data: bytes) -> None:
        self.L = layout
        self.T = trace
        self.cmds = cmds
        self.cmd_src = cmd_src
        self.pairs = pairs
        self.ncells = ncells
        self.name = prog_name
        self.data = data
        self.bg = self._build_background()

    # -- helpers ----------------------------------------------------------

    def cell_cx(self, i: float) -> float:
        L = self.L
        return L.cell_x0 + i * (L.cell_w + L.cell_gap) + L.cell_w / 2

    def _state(self, i: int):
        """(tape, ptr, outlen, inpos) *before* step i; i == len(steps) -> final."""
        T = self.T
        if i >= len(T.steps):
            return T.final_tape, T.final_ptr, len(T.final_out), T.final_inpos
        st = T.steps[i]
        return st.tape, st.ptr, st.outlen, st.inpos

    # -- static background -------------------------------------------------

    def _build_background(self) -> Image.Image:
        L = self.L
        img = Image.new("RGBA", (W * S, H * S))
        # vertical gradient page background
        d = ImageDraw.Draw(img)
        top, bot = BG_TOP, BG_BOT
        for y in range(H * S):
            c = blend(top, bot, y / (H * S))
            d.line([(0, y), (W * S, y)], fill=c + (255,))

        # title
        d.text((36 * S, 30 * S), self.name, font=FONTS.sans(15, True),
               fill=TEXT, anchor="lm")
        d.text((36 * S + FONTS.sans(15, True).getlength(self.name) + 12 * S,
                30 * S),
               "·  brainfuck, step by step", font=FONTS.sans(13),
               fill=DIM, anchor="lm")

        # source card
        r = tuple(v * S for v in L.code_rect)
        d.rounded_rectangle(r, radius=14 * S, fill=CARD, outline=CARD_BRD,
                            width=max(1, S))

        # source text (dim everything that is not a command)
        for sc in L.src:
            if sc.ch == "\n":
                continue
            if sc.cmd >= 0:
                col = AMBER if self.cmds[sc.cmd] in "[]" else TEXT
            else:
                col = FAINT
            d.text((sc.x * S, sc.y * S), sc.ch, font=L.code_font, fill=col,
                   anchor="mm")

        # pointer rail
        rail_y = L.ptr_y * S
        x0 = (L.cell_x0 - 20) * S
        x1 = (self.cell_cx(self.ncells - 1) + L.cell_w / 2 + 20) * S
        d.line([(x0, rail_y), (x1, rail_y)], fill=FAINT, width=S)

        # cells + indices
        for i in range(self.ncells):
            cx = self.cell_cx(i) * S
            y = L.cell_y * S
            w, h = L.cell_w * S, L.cell_h * S
            d.rounded_rectangle((cx - w / 2, y, cx + w / 2, y + h),
                                radius=12 * S, fill=CARD, outline=CARD_BRD,
                                width=max(1, S))
            d.text((cx, y + h + 8 * S), str(i), font=FONTS.mono(11),
                   fill=FAINT, anchor="mt")

        # bottom panel(s)
        for rect in filter(None, (L.out_rect, L.in_rect)):
            r = tuple(v * S for v in rect)
            d.rounded_rectangle(r, radius=14 * S, fill=CARD,
                                outline=CARD_BRD, width=max(1, S))
        self._spaced(d, (L.out_rect[0] + 20) * S, (L.out_rect[1] + 22) * S,
                     "OUTPUT", FONTS.sans(11, True), DIM)
        if L.in_rect:
            self._spaced(d, (L.in_rect[0] + 20) * S, (L.in_rect[1] + 22) * S,
                         "INPUT", FONTS.sans(11, True), DIM)
            sub = (f"{len(self.data)} byte{'s' if len(self.data) != 1 else ''} "
                   f"on stdin" if self.data else "stdin empty — ',' reads EOF")
            d.text((L.in_rect[2] * S - 20 * S, (L.in_rect[1] + 22) * S), sub,
                   font=FONTS.sans(11), fill=FAINT, anchor="rm")
        return img

    @staticmethod
    def _spaced(d: ImageDraw.ImageDraw, x: float, y: float, text: str,
                font, fill, spacing: float = 2.0) -> None:
        for ch in text:
            d.text((x, y), ch, font=font, fill=fill, anchor="lm")
            x += font.getlength(ch) + spacing * S

    # -- dynamic pieces -----------------------------------------------------

    def _draw_status(self, d: ImageDraw.ImageDraw, si: Optional[int]) -> None:
        T = self.T
        n = len(T.steps)
        if si is None:
            msg = f"done — {n} command{'s' if n != 1 else ''} executed"
            if T.final_out:
                msg += f" · {len(T.final_out)} byte{'s' if len(T.final_out) != 1 else ''} out"
        elif si < 0:
            msg = f"ready — {n} command{'s' if n != 1 else ''} · {self.ncells} cells"
        else:
            st = T.steps[si]
            tape, ptr, outlen, inpos = st.tape, st.ptr, st.outlen, st.inpos
            extra = ""
            if st.c in "+-":
                old = tape[ptr]
                new = (old + 1) & 0xFF if st.c == "+" else (old - 1) & 0xFF
                extra = f" · cell {ptr}: {old} → {new}"
            elif st.c in "><":
                nptr = ptr + (1 if st.c == ">" else -1)
                extra = f" · ptr {ptr} → {nptr}"
            elif st.c == ".":
                b = tape[ptr]
                extra = f" · '{byte_glyph(b)}' ({b})"
            elif st.c == ",":
                if st.in_byte is not None and st.in_byte >= 0:
                    extra = f" · '{byte_glyph(st.in_byte)}' ({st.in_byte})"
                else:
                    extra = " · EOF → 0"
            elif st.c == "[":
                extra = " · cell is 0, jump past loop" if st.jump == "skip" \
                        else " · cell ≠ 0, enter loop"
            elif st.c == "]":
                extra = f" · cell ≠ 0, back to cmd #{self.pairs[st.pc] + 1}" \
                        if st.jump == "repeat" else " · cell is 0, exit loop"
            msg = (f"step {si + 1}/{n} · {st.c}  {CMD_DESCR[st.c]}{extra}")
        f = FONTS.mono(13)
        d.text(((W - 36) * S, 30 * S), msg, font=f, fill=DIM, anchor="rm")

    def _code_highlight(self, d: ImageDraw.ImageDraw, si: int,
                        t: float) -> None:
        L = self.L
        st = self.T.steps[si]
        sc = L.src[self.cmd_src[st.pc]]
        cw = L.code_font.getlength("M") / S
        hw, hh = cw / 2 + 3, 11.5
        pill = blend(CARD, AMBER, (90 + 40 * math.sin(math.pi * t)) / 255)
        d.rounded_rectangle(((sc.x - hw) * S, (sc.y - hh) * S,
                             (sc.x + hw) * S, (sc.y + hh) * S),
                            radius=6 * S, fill=pill)
        d.text((sc.x * S, sc.y * S), sc.ch, font=L.code_font,
               fill=(255, 240, 214), anchor="mm")

        # bracket arc between the matching pair
        if st.c in "[]":
            other = L.src[self.cmd_src[self.pairs[st.pc]]]
            env = math.sin(math.pi * t)
            if sc.line == other.line:
                ax, ay = sc.x * S, (sc.y + hh + 2) * S
                bx, by = other.x * S, (other.y + hh + 2) * S
                mx = (ax + bx) / 2
                depth = min(30.0, 8.0 + abs(bx - ax) / S * 0.18) * S
                # quadratic bezier below the line
                pts = []
                for u in [i / 24 for i in range(25)]:
                    x = (1 - u) ** 2 * ax + 2 * (1 - u) * u * mx + u ** 2 * bx
                    y = (1 - u) ** 2 * ay + 2 * (1 - u) * u * (ay + depth) \
                        + u ** 2 * by
                    pts.append((x, y))
                d.line(pts, fill=blend(CARD, AMBER, 0.6 * env), width=2 * S,
                       joint="curve")
            for q in (sc, other):
                d.rounded_rectangle(((q.x - hw) * S, (q.y - hh) * S,
                                     (q.x + hw) * S, (q.y + hh) * S),
                                    radius=6 * S,
                                    outline=blend(CARD, AMBER, 0.65 * env),
                                    width=2 * S)

    def _pointer(self, d: ImageDraw.ImageDraw, idx: int, t: float,
                 active: bool) -> None:
        L = self.L
        _, ptr0, _, _ = self._state(idx)
        if active and self.T.steps[idx].c in "<>":
            _, ptr1, _, _ = self._state(idx + 1)
            x = lerp(self.cell_cx(ptr0), self.cell_cx(ptr1), ease_in_out(t))
        else:
            x = self.cell_cx(ptr0)
        y = L.ptr_y
        d.polygon([(x * S - 9 * S, (y - 13) * S), (x * S + 9 * S, (y - 13) * S),
                   (x * S, (y - 1) * S)], fill=ACCENT)

    def _value_text(self, v: int) -> list[tuple[str, tuple]]:
        """Three odometer digits with leading-zero dimming."""
        s = f"{v:03d}"
        sig = len(str(v))
        out = []
        for i, ch in enumerate(s):
            bright = i >= 3 - sig
            out.append((ch, TEXT if bright else FAINT))
        return out

    def _draw_cells(self, img: Image.Image, d: ImageDraw.ImageDraw,
                    idx: int, t: float, active: bool) -> None:
        L = self.L
        tape0, ptr0, _, _ = self._state(idx)
        tape1, ptr1, _, _ = self._state(idx + 1)
        st = self.T.steps[idx] if active else None
        anim = st is not None and st.c in "+-,"

        # active-cell highlight follows the (possibly sliding) pointer
        if st is not None and st.c in "<>":
            cells_tint = [(ptr0, 1 - ease_in_out(t)), (ptr1, ease_in_out(t))]
        else:
            cells_tint = [(ptr0, 1.0)]

        for ci, amt in cells_tint:
            if amt <= 0.01:
                continue
            cx = self.cell_cx(ci) * S
            y = L.cell_y * S
            w, h = L.cell_w * S, L.cell_h * S
            d.rounded_rectangle((cx - w / 2, y, cx + w / 2, y + h),
                                radius=12 * S,
                                fill=blend(CARD, ACCENT, 0.12 * amt))
            d.rounded_rectangle((cx - w / 2, y, cx + w / 2, y + h),
                                radius=12 * S,
                                outline=blend(CARD, ACCENT, max(0.3, amt)),
                                width=2 * S)

        # values
        vfont = FONTS.mono(26, bold=True)
        dw = vfont.getlength("0")
        roll_h = 30.0 * S                       # one odometer row, 2x px
        for i in range(self.ncells):
            cx = self.cell_cx(i) * S
            cy = (L.cell_y + L.cell_h / 2) * S
            if anim and i == ptr0:
                self._draw_roll(img, cx, cy, dw, roll_h, vfont,
                                tape0[i], tape1[i], st.c, t)
                continue
            digits = self._value_text(tape0[i])
            hot = (i == ptr0)
            for k, (ch, col) in enumerate(digits):
                if hot:
                    col = (255, 255, 255) if col == TEXT else FAINT
                d.text((cx + (k - 1) * dw, cy), ch, font=vfont, fill=col,
                       anchor="mm")

    def _draw_roll(self, img: Image.Image, cx: float, cy: float, dw: float,
                   roll_h: float, vfont, v0: int, v1: int, c: str,
                   t: float) -> None:
        """Odometer roll inside one cell (clipped by pasting a small layer)."""
        L = self.L
        w = int(L.cell_w * S)
        h = int(L.cell_h * S)
        layer = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        ld = ImageDraw.Draw(layer)
        lx, ly = w / 2, h / 2                  # centre in layer coords
        tt = ease_in_out(t)

        if c in "+-" and ((c == "+" and v0 == 255) or (c == "-" and v0 == 0)):
            # 255+1 / 0-1 : roll the whole number like one drum
            s0, s1 = f"{v0:03d}", f"{v1:03d}"
            dy = tt * roll_h * 1.6
            sign = 1 if c == "+" else -1
            ld.text((lx, ly - sign * dy), s0, font=vfont, fill=ACCENT,
                    anchor="mm")
            ld.text((lx, ly + sign * (roll_h * 1.6 - dy)), s1, font=vfont,
                    fill=(255, 255, 255), anchor="mm")
        else:
            d0, d1 = f"{v0:03d}", f"{v1:03d}"
            final_digits = self._value_text(v1)
            # '+' and ',' spin upward (s=+1), '-' rolls downward (s=-1)
            s = -1 if c == "-" else 1
            for k in range(3):
                x = lx + (k - 1) * dw
                a, b = int(d0[k]), int(d1[k])
                if a == b:
                    col = (255, 255, 255) if final_digits[k][1] == TEXT \
                        else FAINT
                    ld.text((x, ly), str(a), font=vfont, fill=col,
                            anchor="mm")
                    continue
                if c == ",":
                    steps = (b - a) % 10        # spin forward to the byte
                else:
                    steps = 1
                for j in range(steps + 1):
                    digit = (a + s * j) % 10
                    y = ly + s * (j - tt * steps) * roll_h
                    col = ACCENT
                    if j == 0:
                        col = blend((255, 255, 255), ACCENT, tt)
                    if j == steps:
                        col = blend(ACCENT, (255, 255, 255), tt)
                    ld.text((x, y), str(digit), font=vfont, fill=col,
                            anchor="mm")
        img.paste(layer, (int(cx - w / 2), int(cy - h / 2)), layer)

    def _draw_output(self, img: Image.Image, d: ImageDraw.ImageDraw,
                     idx: int, t: float, active: bool) -> None:
        L = self.L
        T = self.T
        _, _, outlen, _ = self._state(idx)
        st = self.T.steps[idx] if active else None
        typing = st is not None and st.c == "."

        glyphs: list[tuple[str, tuple, bool]] = []   # (text, color, newline)
        for b in T.final_out[:outlen]:
            if b == 0x0A:
                glyphs.append(("\\n", DIM, True))
            elif b in (0x09, 0x0D) or b < 32 or b >= 127:
                glyphs.append((byte_glyph(b), AMBER, False))
            else:
                glyphs.append((chr(b), GREEN, False))
        new_glyph = None
        if typing:
            b = st.tape[st.ptr]
            appear = clamp01(t / 0.45)
            col = blend((255, 255, 255), GREEN if b == 0x0A or (
                32 <= b < 127) else AMBER, appear)
            new_glyph = (("\\n", blend((255, 255, 255), DIM, appear), True)
                         if b == 0x0A else (byte_glyph(b), col, False))

        # lay glyphs out into lines
        lines: list[list[tuple[str, tuple]]] = [[]]
        for g in glyphs + ([new_glyph] if new_glyph else []):
            text, col, nl = g
            if nl:
                for ch in text:
                    lines[-1].append((ch, col))
                lines.append([])
                continue
            for ch in text:
                if len(lines[-1]) >= L.out_cols:
                    lines.append([])
                lines[-1].append((ch, col))
        vis = lines[-L.out_rows:]
        skip = len(lines) - len(vis)

        cursor_pos = None
        y = L.out_text_y
        for li, line in enumerate(vis):
            x = L.out_text_x
            for ch, col in line:
                d.text((x * S, y * S), ch, font=L.out_font, fill=col,
                       anchor="lm")
                x += L.out_cw
            if li == len(vis) - 1:
                cursor_pos = (x, y)
            y += L.out_lh
        if cursor_pos is None:
            cursor_pos = (L.out_text_x, L.out_text_y)
        cx, cy = cursor_pos
        if new_glyph and t < 0.45:
            # stamp effect: the cursor block covers the fresh character
            cx -= L.out_cw
        d.rectangle(((cx + 1) * S, (cy - 11) * S,
                     (cx + L.out_cw - 1) * S, (cy + 11) * S),
                    fill=blend(CARD, ACCENT, 0.5))

        # byte counter top-right of the panel
        shown = outlen + (1 if typing and t >= 0.45 else 0)
        cnt = f"{shown} byte{'s' if shown != 1 else ''}"
        d.text(((L.out_rect[2] - 20) * S, (L.out_rect[1] + 22) * S), cnt,
               font=FONTS.sans(11), fill=FAINT, anchor="rm")

    def _draw_input(self, img: Image.Image, d: ImageDraw.ImageDraw,
                    idx: int, t: float, active: bool) -> None:
        L = self.L
        if L.in_rect is None:
            return
        T = self.T
        _, _, _, inpos = self._state(idx)
        st = self.T.steps[idx] if active else None
        consuming = st is not None and st.c == ","

        x0 = L.in_rect[0] + 20
        y = L.in_y
        rem = self.data[inpos:]
        chip_h, gap = 30.0, 8.0
        max_x = L.in_rect[2] - 60
        fchip = FONTS.mono(14, bold=True)

        def chip_w(b: int) -> float:
            label = byte_glyph(b) if not (32 <= b < 127) else chr(b)
            return max(26.0, FONTS.mono(14).getlength(label) / S + 14)

        # after the flash, the remaining chips slide left by exactly one slot
        shift = 0.0
        if consuming and t > 0.5 and rem:
            shift = ease_out((t - 0.5) / 0.5) * (chip_w(rem[0]) + gap)

        x = x0
        for k, b in enumerate(rem):
            label = byte_glyph(b) if not (32 <= b < 127) else chr(b)
            cw = chip_w(b)
            xx = x - (shift if k > 0 else 0)
            if xx + cw > max_x:
                d.text((x * S, y * S), f"+{len(rem) - k} more",
                       font=FONTS.sans(12), fill=FAINT, anchor="lm")
                break
            if k == 0 and consuming:
                if t < 0.5:                       # flash the consumed chip
                    p = math.sin(math.pi * t)
                    fill = blend(PURPLE, (255, 255, 255), p * 0.8)
                    outline = (255, 255, 255)
                else:
                    x += cw + gap
                    continue                      # it's gone
            else:
                outline = PURPLE if k == 0 else CARD_BRD
                fill = blend(CARD, PURPLE, 0.28) if k == 0 else CARD
            d.rounded_rectangle((xx * S, (y - chip_h / 2) * S,
                                 (xx + cw) * S, (y + chip_h / 2) * S),
                                radius=8 * S, fill=fill, outline=outline,
                                width=max(1, S))
            d.text(((xx + cw / 2) * S, y * S), label, font=fchip,
                   fill=(20, 24, 34) if (k == 0 and consuming and t < 0.5)
                   else TEXT, anchor="mm")
            x += cw + gap

        if consuming and st.in_byte is not None and st.in_byte < 0:
            d.text(((x0) * S, (y + 26) * S),
                   "EOF — no bytes left, cell gets 0",
                   font=FONTS.sans(12), fill=RED, anchor="lm")
        elif T.eof_hit and idx >= len(T.steps):
            d.text(((x0) * S, (y + 26) * S),
                   "EOF was reached during this run",
                   font=FONTS.sans(12), fill=FAINT, anchor="lm")
        elif not self.data:
            d.text(((x0) * S, y * S),
                   "(no stdin — ',' reads EOF)", font=FONTS.sans(12),
                   fill=FAINT, anchor="lm")

    # -- one frame -----------------------------------------------------------

    def render(self, si: Optional[int], t: float = 0.0) -> Image.Image:
        """si: -1 intro, None outro, otherwise step index; t in [0,1)."""
        if si is None:                    # outro: everything already happened
            idx, active = len(self.T.steps), False
        elif si < 0:                      # intro: untouched initial state
            idx, active = 0, False
        else:
            idx, active = si, True

        img = self.bg.copy()
        d = ImageDraw.Draw(img)

        self._draw_status(d, si)
        if active:
            self._code_highlight(d, idx, t)
        self._pointer(d, idx, t, active)
        self._draw_cells(img, d, idx, t, active)
        self._draw_output(img, d, idx, t, active)
        self._draw_input(img, d, idx, t, active)
        return img


# ----------------------------------------------------------------------------
# GIF assembly
# ----------------------------------------------------------------------------

_PALETTE: Optional[Image.Image] = None


def _build_palette() -> Image.Image:
    """A shared 256-color palette that contains every color the frames use.

    Built from a swatch with the page gradient, all blend ramps and text
    samples, so flat regions quantize exactly and anti-aliased text keeps
    its shades without dither noise.
    """
    sh = 6144
    img = Image.new("RGB", (W * S, sh), CARD)
    d = ImageDraw.Draw(img)
    y = 0
    for i in range(32):                                  # blend ramps
        for ramp in (blend(CARD, AMBER, i / 31), blend(CARD, ACCENT, i / 31),
                     blend(ACCENT, (255, 255, 255), i / 31),
                     blend(PURPLE, (255, 255, 255), i / 31),
                     blend(CARD, PURPLE, i / 31),
                     blend((255, 255, 255), AMBER, i / 31),
                     blend((255, 255, 255), GREEN, i / 31),
                     blend((255, 255, 255), DIM, i / 31),
                     blend(BG_TOP, BG_BOT, i / 31)):
            d.rectangle((0, y, W * S, y + S), fill=ramp)
            y += S
    for col in (CARD, CARD_BRD, TEXT, DIM, FAINT, ACCENT, AMBER, GREEN,
                PURPLE, RED, (255, 255, 255), (255, 240, 214), (20, 24, 34),
                BG_TOP, BG_BOT):
        d.rectangle((0, y, W * S, y + 2 * S), fill=col)
        y += 2 * S
    sample = "Brainfuck 0123456789 +-<>[],. \\n *!"
    for font in (FONTS.mono(16), FONTS.mono(18), FONTS.mono(26, True),
                 FONTS.mono(14, True), FONTS.sans(11, True), FONTS.sans(13),
                 FONTS.mono(13)):
        for col in (TEXT, DIM, FAINT, GREEN, AMBER, ACCENT, PURPLE, RED,
                    (255, 255, 255), (255, 240, 214)):
            d.text((10 * S, y), sample, font=font, fill=col, anchor="lm")
            y += 30 * S
    small = img.resize((W, sh // S), Image.BOX)
    return small.quantize(colors=256, dither=Image.Dither.NONE)


def make_gif(r: Renderer, plan: list[int], frame_ms: int, path: str) -> int:
    cache: dict[tuple, Image.Image] = {}

    def to_p(key: tuple, si: Optional[int], t: float,
             pal: Image.Image) -> Image.Image:
        """Render one frame; static holds (intro/outro) render only once."""
        static = key in ("intro", "outro")
        im = cache.get(key) if static else None
        if im is None:
            im = r.render(si, t).resize((W, H), Image.BOX).convert("RGB") \
                   .quantize(palette=pal, dither=Image.Dither.NONE)
            if static:
                cache[key] = im
        return im

    global _PALETTE
    if _PALETTE is None:
        _PALETTE = _build_palette()
    pal = _PALETTE

    seq: list[tuple[Optional[int], float]] = []
    for _ in range(INTRO_FRAMES):
        seq.append(("intro", -1, 0.0))
    for si, nf in enumerate(plan):
        for k in range(nf):
            seq.append(((si, k), si, (k + 0.5) / nf))
    for _ in range(OUTRO_FRAMES):
        seq.append(("outro", None, 0.0))

    it = iter(seq)
    key, si, t = next(it)
    first = to_p(key, si, t, pal)

    def gen() -> Iterator[Image.Image]:
        for key, si, t in it:
            yield to_p(key, si, t, pal)

    first.save(path, save_all=True, append_images=gen(), duration=frame_ms,
               loop=0, optimize=False)
    cache.clear()
    return len(seq)


# ----------------------------------------------------------------------------
# CLI
# ----------------------------------------------------------------------------

def main(argv: Optional[list[str]] = None) -> int:
    ap = argparse.ArgumentParser(
        prog="bfviz",
        description="Render a step-by-step animated GIF of a Brainfuck "
                    "program (code from a file, input from stdin).")
    ap.add_argument("program", help="path to the .bf source file")
    ap.add_argument("-o", "--output", default=None,
                    help="output GIF path (default: <program>.gif)")
    ap.add_argument("--cells", type=int, default=10,
                    help="tape size, 4..16 (default 10)")
    ap.add_argument("--max-steps", type=int, default=DEF_MAX_STEPS,
                    help=f"abort after this many commands "
                         f"(default {DEF_MAX_STEPS})")
    ap.add_argument("--max-frames", type=int, default=DEF_MAX_FRAMES,
                    help=f"GIF frame budget (default {DEF_MAX_FRAMES})")
    ap.add_argument("--frame-ms", type=int, default=DEF_FRAME_MS,
                    help=f"delay per frame in ms (default {DEF_FRAME_MS})")
    args = ap.parse_args(argv)

    if not 4 <= args.cells <= 16:
        ap.error("--cells must be between 4 and 16")
    if args.frame_ms < 20:
        ap.error("--frame-ms below 20 is clamped to 100 ms by most GIF "
                 "renderers; keep it >= 20")

    try:
        chars, cmds, cmd_src, _ = load_source(args.program)
        pairs = match_brackets(cmds, chars, cmd_src)

        if sys.stdin.isatty():
            sys.stderr.write("note: stdin is a terminal — running with empty "
                             "input (redirect a file to feed ',')\n")
            data = b""
        else:
            data = sys.stdin.buffer.read()

        trace = execute(cmds, pairs, data, args.cells, args.max_steps)
        plan, scaled = frames_per_step(trace, args.max_frames)
        layout = compute_layout(chars, args.cells, "," in cmds)
        name = os.path.basename(args.program)
        r = Renderer(layout, trace, cmds, cmd_src, pairs, args.cells, name,
                     data)
        out = args.output or os.path.splitext(args.program)[0] + ".gif"
        frame_ms = args.frame_ms
        n = INTRO_FRAMES + sum(plan) + OUTRO_FRAMES
        if n * frame_ms > 40_000:          # keep long traces watchable
            frame_ms = max(30, int(40_000 / n))
            sys.stderr.write(f"note: long trace, frame delay lowered to "
                             f"{frame_ms} ms\n")
        n = make_gif(r, plan, frame_ms, out)
        dur = n * frame_ms / 1000
        sys.stderr.write(
            f"wrote {out} — {n} frames, {W}x{H}, ~{dur:.0f}s, "
            f"{len(trace.steps)} commands executed"
            f"{', loop frames scaled down' if scaled else ''}\n")
        return 0
    except BfError as e:
        sys.stderr.write(f"error: {e}\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
