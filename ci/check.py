"""CI-side validation of the rendered GIFs (runs on the Actions runner)."""
import os

from PIL import Image

os.makedirs("/tmp/frames", exist_ok=True)

EXPECT = {
    "add":   {"min_frames": 100, "max_s": 45},
    "hello": {"min_frames": 100, "max_s": 60},
    "stars": {"min_frames": 100, "max_s": 60},
    "cat":   {"min_frames": 30,  "max_s": 20},
}

for name, ex in EXPECT.items():
    im = Image.open(f"/tmp/{name}.gif")
    assert im.size == (1280, 720), (name, im.size)
    n = 0
    tot = 0
    try:
        while True:
            tot += im.info.get("duration", 0)
            n += 1
            im.seek(im.tell() + 1)
    except EOFError:
        pass
    assert n >= ex["min_frames"], (name, "too few frames", n)
    assert tot / 1000 <= ex["max_s"], (name, "too long", tot)
    for tag, i in {"intro": 0, "a": n // 4, "b": n // 2, "c": 3 * n // 4,
                   "outro": n - 1}.items():
        im.seek(i)
        im.convert("RGB").save(f"/tmp/frames/{name}_{tag}.png")
    print(f"{name}: {n} frames, {tot/1000:.0f}s ok")

print("all gifs ok")
