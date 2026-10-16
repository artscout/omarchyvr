#version 450
// The eye-calibration scene, analytically per pixel (src/gpu.rs fills the uniform block, src/gpu.rs `shade` is the
// CPU twin used by the tests): a background colour and up to MAX_MARKS marks, each given by its direction in THIS
// EYE's view space (the CPU does the head-to-eye transform, so canted views need nothing here), an angular radius,
// a colour and a kind: 0 disc, 1 ring (width), 2 cross (two bars of the given width, half length = radius).
// Anti-aliased by the pixel's own angular size. Output is the panel code converted with srgb_to_linear, which gives
// the right bytes for both *_SRGB (re-encoded by the hardware) and *_UNORM attachments.
layout(location = 0) out vec4 outColor;

const int MAX_MARKS = 32;
struct Mark {
    vec4 dir_r;      // unit direction (eye space: x right, y up, -z forward), radius (rad)
    vec4 color_kind; // sRGB colour, kind
    vec4 extra;      // width (rad), unused
};
layout(std140, set = 0, binding = 0) uniform Scene {
    vec4 tan_;   // tangents of the half-angles: left, right, up, down (positive)
    vec4 info;   // width, height, tangent per pixel, mark count
    vec4 bg;     // background sRGB
    Mark marks[MAX_MARKS];
} u;

float srgb_to_linear(float c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

float cover(float d, float pix) { return clamp(d / pix + 0.5, 0.0, 1.0); }

void main() {
    vec2 px = gl_FragCoord.xy;
    float w = u.info.x, h = u.info.y;
    float tl = u.tan_.x, tr = u.tan_.y, tu = u.tan_.z, td = u.tan_.w;
    float tx = -tl + px.x / w * (tl + tr);
    float ty = tu - px.y / h * (tu + td);
    float r2 = tx * tx + ty * ty;
    float n = sqrt(r2 + 1.0);
    vec3 v = vec3(tx / n, ty / n, -1.0 / n);
    float pix = u.info.z / (1.0 + r2);
    vec3 code = u.bg.rgb;
    int count = min(int(u.info.w + 0.5), MAX_MARKS);
    for (int i = 0; i < count; i++) {
        vec3 d = u.marks[i].dir_r.xyz;
        float r = u.marks[i].dir_r.w;
        int kind = int(u.marks[i].color_kind.w + 0.5);
        float wd = u.marks[i].extra.x;
        float c = dot(d, v);
        // cheap reject: further than radius + width + 4 pixels
        if (c < cos(min(r + wd + 4.0 * pix, 3.0))) continue;
        float ang = atan(length(cross(d, v)), c);
        float cov = 0.0;
        if (kind == 0) {
            cov = cover(r - ang, pix);
        } else if (kind == 1) {
            cov = cover(0.5 * wd - abs(ang - r), pix);
        } else {
            vec3 e1 = normalize(cross(d, vec3(0.0, 1.0, 0.0)));
            vec3 e2 = cross(e1, d);
            float lx = dot(v, e1) / c;
            float ly = dot(v, e2) / c;
            float b1 = min(cover(0.5 * wd - abs(ly), pix), cover(r - abs(lx), pix));
            float b2 = min(cover(0.5 * wd - abs(lx), pix), cover(r - abs(ly), pix));
            cov = max(b1, b2);
        }
        code = mix(code, u.marks[i].color_kind.rgb, cov);
    }
    outColor = vec4(srgb_to_linear(code.r), srgb_to_linear(code.g), srgb_to_linear(code.b), 1.0);
}
