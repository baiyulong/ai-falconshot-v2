# P6: geometry check on the scene-graph grabs. The selection border is drawn white,
# so the undimmed rect's device-pixel bounds can be read straight off the image and
# compared against the rect the probe declared. Aggregate ratios alone would not catch
# a shader whose hole is the right size but in the wrong place... this does.
import sys
from PIL import Image

DECLARED = (2067, 1200, 2880, 1708)  # hole=(x0,y0)-(x1,y1) from the [P1] dim-check line


def bbox(path):
    img = Image.open(path).convert("RGB")
    px = img.load()
    w, h = img.size
    rows, cols = [], []
    for y in range(h):
        run = 0
        best = 0
        for x in range(w):
            r, g, b = px[x, y]
            run = run + 1 if min(r, g, b) > 245 else 0
            best = best if best > run else run
        if best > 100:
            rows.append((y, best))
    for x in range(w):
        run = 0
        best = 0
        for y in range(h):
            r, g, b = px[x, y]
            run = run + 1 if min(r, g, b) > 245 else 0
            best = best if best > run else run
        if best > 100:
            cols.append((x, best))
    return img.size, rows, cols


for path in sys.argv[1:]:
    size, rows, cols = bbox(path)
    top, bottom = rows[0][0], rows[-1][0]
    left, right = cols[0][0], cols[-1][0]
    print("%-28s size=%dx%d border=(%d,%d)-(%d,%d) declared=(%d,%d)-(%d,%d) delta=(%d,%d,%d,%d)"
          % (path.split("/")[-1], size[0], size[1], left, top, right, bottom, *DECLARED,
             left - DECLARED[0], top - DECLARED[1], right - DECLARED[2], bottom - DECLARED[3]))
    print("   rows with a long white run: %d..%d (%d of them), cols: %d..%d (%d)"
          % (top, bottom, len(rows), left, right, len(cols)))
