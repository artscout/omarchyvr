#version 450
layout(location = 0) out vec2 uv;
void main() {
	// full-screen triangle; uv (0,0) = top-left of the image, (1,1) = bottom-right
	vec2 p = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
	uv = p;
	gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
