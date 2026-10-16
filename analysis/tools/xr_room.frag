#version 450
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 outc;
layout(push_constant) uniform PC {
	vec4 c0, c1, c2;  // columns of the view->world rotation matrix
	vec4 tan_lrud;    // tan(angleLeft), tan(angleRight), tan(angleUp), tan(angleDown)
} pc;

const float PI = 3.14159265;

// distance-based line mask with ~1.5px AA: x in "cell units", w = fwidth
float gridline(float x, float w, float thick) {
	float d = abs(fract(x + 0.5) - 0.5);
	return 1.0 - smoothstep(thick * w, (thick + 1.5) * w, d);
}

// letter F on a [0,1]x[0,1] cell (y up)
float letterF(vec2 p) {
	float m = 0.0;
	m += step(0.25, p.x) * step(p.x, 0.40) * step(0.15, p.y) * step(p.y, 0.85);
	m += step(0.25, p.x) * step(p.x, 0.75) * step(0.70, p.y) * step(p.y, 0.85);
	m += step(0.25, p.x) * step(p.x, 0.65) * step(0.45, p.y) * step(p.y, 0.58);
	return clamp(m, 0.0, 1.0);
}

void main() {
	vec3 dv = vec3(mix(pc.tan_lrud.x, pc.tan_lrud.y, uv.x), mix(pc.tan_lrud.z, pc.tan_lrud.w, uv.y), -1.0);
	vec3 d = normalize(mat3(pc.c0.xyz, pc.c1.xyz, pc.c2.xyz) * dv);
	// world: +X right, +Y up, -Z front
	float lon = degrees(atan(d.x, -d.z));      // 0 front, +90 right, +-180 back
	float lat = degrees(asin(clamp(d.y, -1.0, 1.0)));
	float coslat = max(cos(radians(lat)), 0.05);

	vec3 col;
	if (d.y >= 0.0) {
		col = mix(vec3(0.35, 0.50, 0.75), vec3(0.03, 0.05, 0.20), pow(clamp(d.y, 0.0, 1.0), 0.6)); // sky
	} else {
		float t = 1.6 / max(-d.y, 0.001);              // floor 1.6 m below the head
		vec2 p = d.xz * t;
		vec2 f = floor(p);
		float ck = mod(f.x + f.y, 2.0);
		vec3 fl = mix(vec3(0.20), vec3(0.45), ck);
		float dist = length(p);
		// floor tile lines
		vec2 fw = fwidth(p);
		vec2 q = abs(fract(p) - 0.5);
		fl = mix(fl, fl * 0.6, 0.0);
		col = mix(fl, vec3(0.30, 0.32, 0.36), clamp(dist / 40.0, 0.0, 1.0));
		// moire guard: far away the checker averages to grey
		float far = clamp(max(fw.x, fw.y) * 0.7, 0.0, 1.0);
		col = mix(col, vec3(0.32), far);
	}

	// lat/lon grid every 15 degrees
	float wl = min(fwidth(lon), 3.0), wa = fwidth(lat);
	float gl = gridline(lon / 15.0, wl / 15.0, 0.5) * smoothstep(0.15, 0.5, coslat);
	float ga = gridline(lat / 15.0, wa / 15.0, 0.5);
	float g = max(gl, ga);
	col = mix(col, vec3(0.9), g * 0.65);
	// every 90 degrees of longitude: thicker, brighter
	float g90 = gridline(lon / 90.0, min(wl, 3.0) / 90.0, 1.0) * smoothstep(0.15, 0.5, coslat);
	col = mix(col, vec3(1.0), g90 * 0.8);

	// horizon line (thick, bright orange)
	float hz = 1.0 - smoothstep(1.0, 1.0 + 1.5 * wa, abs(lat));
	col = mix(col, vec3(1.0, 0.55, 0.05), hz);

	// cardinal markers: angular discs of radius 9 degrees
	vec3 dirs[6] = vec3[](vec3(0, 0, -1), vec3(0, 0, 1), vec3(-1, 0, 0), vec3(1, 0, 0), vec3(0, 1, 0), vec3(0, -1, 0));
	vec3 cols[6] = vec3[](vec3(0.1, 0.9, 0.2), vec3(0.9, 0.1, 0.1), vec3(0.15, 0.3, 1.0), vec3(1.0, 0.9, 0.1), vec3(1.0), vec3(0.5));
	for (int i = 0; i < 6; i++) {
		float c = dot(d, dirs[i]);
		float ang = degrees(acos(clamp(c, -1.0, 1.0)));
		float aw = fwidth(ang);
		float disc = 1.0 - smoothstep(9.0, 9.0 + 1.5 * aw, ang);
		float ring = (1.0 - smoothstep(10.5, 10.5 + 1.5 * aw, ang)) * (1.0 - disc);
		vec3 mc = cols[i];
		if (i == 0 && c > 0.0) {
			// letter F in the front marker (tangent-plane coords scaled to the disc)
			vec2 lp = vec2(d.x, d.y) / c / tan(radians(9.0));   // -1..1 across the disc
			float f = letterF(lp * 0.5 + 0.5) * step(abs(lp.x), 1.0) * step(abs(lp.y), 1.0);
			mc = mix(mc, vec3(0.0), f);
		}
		col = mix(col, mc, disc);
		col = mix(col, vec3(0.0), ring * 0.8);
	}

	outc = vec4(col, 1.0);
}
