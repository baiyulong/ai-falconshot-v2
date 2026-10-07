#version 440

// The mask's dim-with-a-hole in one pass: it samples the frozen desktop and
// writes either the desktop pixel or that pixel dimmed, so the whole mask is a
// single quad rather than the image plus four dim rectangles. Only a fragment
// shader is given, so Qt's default vertex shader supplies qt_Matrix and
// qt_TexCoord0.
//
// Chosen over the four-rectangle drawing for shape expressiveness, not speed -
// the two were measured indistinguishable (plan §3.5① ⑧), and rounded or
// multi-part selections cannot be assembled from rectangles. The rectangle
// drawing stays in `CaptureMask.qml` as the `software` RHI fallback, because
// ShaderEffect needs an RHI and that combination was never measured.

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec4 dimColor;      // rgb ignored, a = how strongly to dim toward black
    vec4 hole;          // x, y, w, h in device-independent pixels
    vec2 deviceSize;    // the item's size in device pixels
    float dpr;
} ubuf;

layout(binding = 1) uniform sampler2D src;

void main() {
    // gl_FragCoord is bottom-left origin; the hole rect is top-left like
    // everything else in QML, so flip before the test.
    vec2 dip = vec2(gl_FragCoord.x, ubuf.deviceSize.y - gl_FragCoord.y) / ubuf.dpr;
    float inside = step(ubuf.hole.x, dip.x) * step(dip.x, ubuf.hole.x + ubuf.hole.z)
                 * step(ubuf.hole.y, dip.y) * step(dip.y, ubuf.hole.y + ubuf.hole.w);
    vec4 img = texture(src, qt_TexCoord0);
    float k = ubuf.dimColor.a * (1.0 - inside);
    fragColor = vec4(img.rgb * (1.0 - k), img.a) * ubuf.qt_Opacity;
}
