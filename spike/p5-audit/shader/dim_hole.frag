#version 440
layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;
layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec4 dimColor;
    vec4 hole;   // x, y, w, h in fragment coordinates
} ubuf;
void main() {
    vec2 p = gl_FragCoord.xy;
    float inside = step(ubuf.hole.x, p.x) * step(p.x, ubuf.hole.x + ubuf.hole.z)
                 * step(ubuf.hole.y, p.y) * step(p.y, ubuf.hole.y + ubuf.hole.w);
    fragColor = mix(ubuf.dimColor, vec4(0.0), inside) * ubuf.qt_Opacity;
}
