# P6: are the two dim variants pixel-equivalent? Compare the two scene-graph grabs.
# The desktop behind them is only nearly static (a clock, animated tiles), so the
# question is where the difference lives, not merely how much of it there is.
from PIL import Image, ImageChops

a = Image.open("p6-grab-rects.png").convert("RGB")
b = Image.open("p6-grab-shader.png").convert("RGB")
print("sizes", a.size, b.size)
d = ImageChops.difference(a, b)
hist = d.histogram()
n = a.size[0] * a.size[1]
tot = sum(i * c for i, c in enumerate(hist[:256]))
print("mean abs diff per channel: %.4f  (%.4f%% of 255)" % (tot / (3 * n), 100.0 * tot / (3 * n) / 255))
print("per-channel extrema:", d.getextrema())
for t in (4, 16, 64):
    r, g, bl = d.split()
    mx = ImageChops.lighter(ImageChops.lighter(r, g), bl)  # per-pixel max channel diff
    h = mx.histogram()
    bad = sum(h[t + 1 :])
    print("  pixels with max channel diff > %2d : %8d  (%.5f%%)" % (t, bad, 100.0 * bad / n))
r, g, bl = d.split()
mx = ImageChops.lighter(ImageChops.lighter(r, g), bl)
print("bbox of pixels differing by > 16:", mx.point(lambda p: 255 if p > 16 else 0).getbbox())
