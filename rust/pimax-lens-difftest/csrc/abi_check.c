/* C program linked against libpimax_lens.a only through include/pimax_lens_rs.h: prints what the ABI returns so the
 * Rust test can compare it with the library used directly. Exit codes 10.. = ABI misbehaviour. */
#include <stdio.h>
#include <string.h>

#include "pimax_lens_rs.h"

static int log_lines, warn_lines;

static void logcb(void *user, int level, const char *msg)
{
	(void)user;
	log_lines++;
	if (level == 1) {
		warn_lines++;
	}
	printf("log %d %s\n", level, msg);
}

int main(void)
{
	if (pimax_lens_rs_abi_version() != PIMAX_LENS_RS_ABI_VERSION) {
		return 10;
	}
	pimax_lens_rs *l = pimax_lens_rs_create(3840.0f / 3552.0f, logcb, NULL);
	if (l == NULL) {
		return 11;
	}
	static const float pts[][2] = {{0.5f, 0.5f}, {0.1f, 0.2f}, {0.9f, 0.8f}, {0.0f, 0.0f}, {1.0f, 1.0f}, {0.3333f, 0.7777f}};
	for (unsigned view = 0; view < 2; view++) {
		float fov[4];
		if (pimax_lens_rs_get_fov(l, view, fov) != 0) {
			return 12;
		}
		printf("fov %u %.9g %.9g %.9g %.9g\n", view, fov[0], fov[1], fov[2], fov[3]);
		for (unsigned i = 0; i < sizeof(pts) / sizeof(pts[0]); i++) {
			float uv[6];
			if (pimax_lens_rs_compute(l, view, pts[i][0], pts[i][1], uv) != 0) {
				return 13;
			}
			printf("uv %u %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g\n", view, pts[i][0], pts[i][1], uv[0], uv[1], uv[2], uv[3],
			       uv[4], uv[5]);
		}
		int has = pimax_lens_rs_has_mask(l, view);
		printf("mask %u %d\n", view, has);
		for (int kind = 0; kind < 3; kind++) {
			uint32_t counts[2] = {0, 0};
			int r = pimax_lens_rs_visibility_mask(l, view, kind, NULL, 0, NULL, 0, counts);
			if (has ? r != -2 : r != -3) {
				return 14;
			}
			if (has) {
				float verts[2 * 512];
				uint32_t idx[2048];
				if (counts[0] > 512 || counts[1] > 2048) {
					return 15;
				}
				r = pimax_lens_rs_visibility_mask(l, view, kind, verts, 512, idx, 2048, counts);
				if (r != 0) {
					return 16;
				}
				printf("meshcounts %u %d %u %u\n", view, kind, counts[0], counts[1]);
			}
		}
	}
	if (pimax_lens_rs_get_fov(l, 2, (float[4]){0}) != -1 || pimax_lens_rs_compute(NULL, 0, 0, 0, (float[6]){0}) != -1) {
		return 17;
	}
	pimax_lens_rs_destroy(l);
	pimax_lens_rs_destroy(NULL);
	printf("done %d %d\n", log_lines, warn_lines);
	return 0;
}
