/* Glue around the renamed original lens model: opaque handle, flat arrays, captured log. */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "pimax_lens.h" /* declares c_pimax_lens_* (renamed by -D in build.rs) */

static char log_buf[1 << 16];
static size_t log_len;

void shim_log(int level, const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	size_t room = sizeof(log_buf) - log_len;
	int n = vsnprintf(log_buf + log_len, room, fmt, ap);
	va_end(ap);
	if (n > 0 && (size_t)n < room) {
		log_len += (size_t)n;
		if (log_len + 2 < sizeof(log_buf)) {
			log_buf[log_len++] = level ? 'W' : 'I'; /* level marker, then newline */
			log_buf[log_len++] = '\n';
			log_buf[log_len] = 0;
		}
	}
}

void shim_log_clear(void)
{
	log_len = 0;
	log_buf[0] = 0;
}

const char *shim_log_get(void)
{
	return log_buf;
}

void *shim_lens_new(void)
{
	return calloc(1, sizeof(struct pimax_lens));
}

void shim_lens_free(void *l)
{
	free(l);
}

void shim_lens_load(void *l, float aspect)
{
	pimax_lens_load((struct pimax_lens *)l, aspect);
}

int shim_lens_enabled(void *l)
{
	return ((struct pimax_lens *)l)->enabled;
}

void shim_lens_get_fov(void *l, unsigned view, float out[4])
{
	struct xrt_fov f = pimax_lens_get_fov((struct pimax_lens *)l, view);
	out[0] = f.angle_left;
	out[1] = f.angle_right;
	out[2] = f.angle_up;
	out[3] = f.angle_down;
}

void shim_lens_compute(void *l, unsigned view, float u, float v, float out[6])
{
	struct xrt_uv_triplet t;
	pimax_lens_compute((struct pimax_lens *)l, view, u, v, &t);
	out[0] = t.r.x; out[1] = t.r.y;
	out[2] = t.g.x; out[3] = t.g.y;
	out[4] = t.b.x; out[5] = t.b.y;
}
