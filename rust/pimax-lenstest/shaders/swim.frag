#version 450
// GPU version of src/patterns/swim.rs (`shade` / `shade_ray`): keep the two in step (a test compares them).
// One full-screen pass per eye: pixel -> tangent -> tuning warp (src/tune.rs `Warp::apply`) -> world ray ->
// 4 m cube room (grid lines, horizon, fixation cross) and poles, analytically, with the same coverage
// anti-aliasing as the CPU. Output is the PANEL CODE converted with srgb_to_linear, which gives the right bytes
// for both an *_SRGB attachment (hardware re-encodes) and an *_UNORM one (stored as is), as Encoding does.
layout(location = 0) out vec4 outColor;
layout(set = 0, binding = 0) uniform sampler2D hudTex;

layout(push_constant) uniform PC {
    vec4 rot0;  // eye -> room rotation, rows
    vec4 rot1;
    vec4 rot2;
    vec4 pos;   // eye position in the room (xyz)
    vec4 tan_;  // tangents of the half-angles: left, right, up, down (positive)
    vec4 warp;  // s, dk, g, dk2
    vec4 info;  // width, height, eye (0/1), tangent per pixel
    vec4 hud;   // y0, y1 (rows of the HUD strip, y1 <= y0: none), blank (1: black eye), unused
} pc;

const float HALF = 2.0;
const float GRID = 0.25;
const int MAJOR_EVERY = 4;
const float POLE_DIST = 1.5;
const float POLE_RADIUS = 0.006;
const float BUMP_U = 0.7;
const float AZ[7] = float[7](-60.0, -40.0, -20.0, 0.0, 20.0, 40.0, 60.0);

float cover(float d, float hw, float pix) { return clamp((hw - abs(d)) / pix + 0.5, 0.0, 1.0); }
vec3 mixc(vec3 a, vec3 b, float t) { return mix(a, b, clamp(t, 0.0, 1.0)); }
vec3 gray(float v) { return vec3(v); }

vec3 eye_background(int eye) {
    return eye == 0 ? vec3(0.02, 0.04, 0.09) : vec3(0.09, 0.03, 0.03);
}

vec3 pole_color(float az) {
    int a = int(abs(az));
    if (a == 0) return vec3(1.0, 0.0, 0.0);
    if (a == 20) return vec3(1.0, 1.0, 0.0);
    if (a == 40) return vec3(1.0, 0.5, 0.0);
    return vec3(0.2, 0.9, 1.0);
}

// coverage of the grid line nearest to v, foot = pixel size in metres there
vec2 line_cover(float v, float foot) {
    float i = round(v / GRID);
    bool major = (int(i) % MAJOR_EVERY) == 0;  // (int(i) % 4 == 0 for negatives too)
    float hw = foot * (major ? 1.4 : 0.7);
    return vec2(cover(v - i * GRID, hw, foot), major ? 1.0 : 0.0);
}

vec3 shade_ray(vec3 o, vec3 d, float ang, int eye) {
    float t_wall = 3.402823e38;
    int axis = 0;
    for (int i = 0; i < 3; i++) {
        float t = 3.402823e38;
        if (d[i] > 1e-7) t = (HALF - o[i]) / d[i];
        else if (d[i] < -1e-7) t = (-HALF - o[i]) / d[i];
        if (t > 0.0 && t < t_wall) { t_wall = t; axis = i; }
    }
    if (t_wall == 3.402823e38) return eye_background(eye);
    vec3 h = o + d * t_wall;
    float foot = t_wall * ang / max(abs(d[axis]), 0.15);
    int a = axis == 0 ? 1 : 0;
    int b = axis == 2 ? 1 : 2;
    vec3 base = eye_background(eye);
    vec3 c = base * 0.8;
    vec2 la = line_cover(h[a], foot);
    vec2 lb = line_cover(h[b], foot);
    c = mixc(c, la.y > 0.5 ? gray(0.85) : gray(0.36), la.x);
    c = mixc(c, lb.y > 0.5 ? gray(0.85) : gray(0.36), lb.x);
    if (axis != 1) c = mixc(c, vec3(0.2, 0.75, 0.3), cover(h[1], foot * 1.2, foot) * 0.9);
    if (axis == 2 && d[2] < 0.0) {
        float w = foot * 1.6;
        if (abs(h[1]) < 0.08) c = mixc(c, vec3(1.0, 0.0, 1.0), cover(h[0], w, foot));
        if (abs(h[0]) < 0.08) c = mixc(c, vec3(1.0, 0.0, 1.0), cover(h[1], w, foot));
    }
    float best = 3.402823e38;
    float h2 = d.x * d.x + d.z * d.z;
    if (h2 >= 1e-9) {
        for (int k = 0; k < 7; k++) {
            float az = AZ[k];
            float s = sin(radians(az)), co = cos(radians(az));
            float cx = POLE_DIST * s, cz = -POLE_DIST * co;
            float tc = ((cx - o.x) * d.x + (cz - o.z) * d.z) / h2;
            if (tc <= 0.0 || tc >= t_wall || tc >= best) continue;
            float px = o.x + d.x * tc - cx;
            float pz = o.z + d.z * tc - cz;
            float dist = length(vec2(px, pz));
            float f = tc * ang / sqrt(h2);
            float cv = cover(dist, POLE_RADIUS, max(f, 1e-6));
            if (cv > 0.0) { c = mixc(c, pole_color(az), cv); best = tc; }
        }
    }
    return c;
}

float srgb_to_linear(float c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

void main() {
    int eye = int(pc.info.z + 0.5);
    vec3 code = vec3(0.0);
    ivec2 px = ivec2(gl_FragCoord.xy);
    if (pc.hud.z < 0.5) {
        float w = pc.info.x, h = pc.info.y;
        float tl = pc.tan_.x, tr = pc.tan_.y, tu = pc.tan_.z, td = pc.tan_.w;
        float tx = -tl + (float(px.x) + 0.5) / w * (tl + tr);
        float ty = tu - (float(px.y) + 0.5) / h * (tu + td);
        // tuning warp, as Warp::apply
        float s = pc.warp.x, dk = pc.warp.y, g = pc.warp.z, dk2 = pc.warp.w;
        if (s != 1.0 || dk != 0.0 || dk2 != 0.0) {
            float u = length(vec2(tx, ty)) / g;
            float a = (u / BUMP_U) * (u / BUMP_U);
            float f = s * (1.0 + dk * u * u + dk2 * a * exp(1.0 - a));
            tx *= f;
            ty *= f;
        }
        float n = sqrt(tx * tx + ty * ty + 1.0);
        vec3 v = vec3(tx / n, ty / n, -1.0 / n);
        vec3 d = vec3(dot(pc.rot0.xyz, v), dot(pc.rot1.xyz, v), dot(pc.rot2.xyz, v));
        float r2 = tx * tx + ty * ty;
        float ang = pc.info.w / (1.0 + r2);
        code = shade_ray(pc.pos.xyz, d, ang, eye);
        float y0 = pc.hud.x, y1 = pc.hud.y;
        if (float(px.y) >= y0 && float(px.y) < y1) {
            vec3 t = texelFetch(hudTex, ivec2(px.x, px.y - int(y0)), 0).rgb;
            if (t.r + t.g + t.b > 0.0) code = t;
        }
    }
    outColor = vec4(srgb_to_linear(code.r), srgb_to_linear(code.g), srgb_to_linear(code.b), 1.0);
}
