#version 440

// The mask's dim-with-a-hole in one pass: it samples the frozen desktop and
// writes either the desktop pixel or that pixel dimmed, so the whole mask is a
// single quad rather than the image plus four dim rectangles. Only a fragment
// shader is given, so Qt's default vertex shader supplies qt_Matrix and
// qt_TexCoord0.
//
// Chosen over the four-rectangle drawing for shape expressiveness, not speed -
// the two were measured indistinguishable, and rounded or
// multi-part selections cannot be assembled from rectangles. The rectangle
// drawing stays in `CaptureMask.qml` as the `software` RHI fallback, because
// ShaderEffect needs an RHI and that combination was never measured.

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec4 dimColor;      // rgb ignored, a = how strongly to dim toward black
    vec4 hole;          // x, y, width, height in device-independent pixels
    vec2 deviceSize;    // the item's size in device pixels
    float dpr;
} ubuf;

layout(binding = 1) uniform sampler2D src;

void main() {
    // The window-local position of this pixel, in device-independent pixels and
    // with the origin at the top left - the same space `hole` is stated in
    // (`CaptureMask.qml` hands it over as `hole_x/hole_y/hole_w/hole_h`, which
    // `MaskViewData::hole` already converted from desktop pixels).
    //
    // **The texture coordinate is what carries it, not `gl_FragCoord`.**
    // `gl_FragCoord` is the fragment's position in the framebuffer, and which
    // corner that framebuffer counts from is the RHI's business, not GLSL's:
    // measured on this machine (2026-11-12), the RHI hands it over counting
    // from the top while the GLSL convention counts from the bottom, so the
    // `deviceSize.y - gl_FragCoord.y` flip turned the hole's vertical test
    // upside down. The hole then landed mirrored, away from the selection, and
    // the mask showed one evenly dimmed screen with no bright rectangle inside
    // it - which is what "the highlight is not inside the selection" reads as
    // from the outside.
    //
    // `qt_TexCoord0` is the texture coordinate of this quad: `[0,1]` across the
    // item, with `v` counting **downwards** (the same way the item's own `y`
    // does, and the same way the frozen desktop image is sampled a line
    // below). Multiplied by the item's size in device pixels and divided by the
    // scale, it is the window-local position in device-independent pixels -
    // no flip, and no dependence on which corner the framebuffer counts from.
    //
    // The four-rectangle fallback in `CaptureMask.qml` states the same hole in
    // QML coordinates, which is why the software RHI path draws the hole where
    // the selection is (measured: the selection's area stays at full
    // brightness while the rest of the screen sits at 1 - DIM).
    vec2 dip = qt_TexCoord0 * (ubuf.deviceSize / ubuf.dpr);
    float inside = step(ubuf.hole.x, dip.x) * step(dip.x, ubuf.hole.x + ubuf.hole.z)
                 * step(ubuf.hole.y, dip.y) * step(dip.y, ubuf.hole.y + ubuf.hole.w);
    vec4 img = texture(src, qt_TexCoord0);
    float k = ubuf.dimColor.a * (1.0 - inside);
    fragColor = vec4(img.rgb * (1.0 - k), img.a) * ubuf.qt_Opacity;
}
