/* C side of tests/c_abi.rs: struct layouts as a C compiler sees include/pimax_cam.h, then calibration and a
 * replay driven through the C API. */
#include "pimax_cam.h"
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>

#define O(t, f) (unsigned long)offsetof(t, f)

int
main(int argc, char **argv)
{
	printf("frame_info %zu %lu %lu %lu %lu %lu %lu %lu %lu %lu\n", sizeof(pimax_cam_frame_info),
	       O(pimax_cam_frame_info, host_ts_ns), O(pimax_cam_frame_info, dev_ts_us), O(pimax_cam_frame_info, mono_ts_ns),
	       O(pimax_cam_frame_info, counter), O(pimax_cam_frame_info, kind), O(pimax_cam_frame_info, exposure),
	       O(pimax_cam_frame_info, y_offset), O(pimax_cam_frame_info, tile_width), O(pimax_cam_frame_info, flags));
	printf("stats %zu %lu %lu\n", sizeof(pimax_cam_stats), O(pimax_cam_stats, kind_repeats),
	       O(pimax_cam_stats, clock_offset_ns));
	printf("clockref %zu %lu %lu\n", sizeof(pimax_cam_imu_clock_ref), O(pimax_cam_imu_clock_ref, dev_ext_us),
	       O(pimax_cam_imu_clock_ref, offset_ns));
	printf("kb4 %zu %lu %lu %lu\n", sizeof(pimax_cam_kb4), O(pimax_cam_kb4, fx), O(pimax_cam_kb4, k),
	       O(pimax_cam_kb4, extra));
	printf("camera %zu %lu %lu %lu %lu %lu\n", sizeof(pimax_cam_camera), O(pimax_cam_camera, id),
	       O(pimax_cam_camera, intr), O(pimax_cam_camera, rig_rotation), O(pimax_cam_camera, rig_translation),
	       O(pimax_cam_camera, time_delta_s));
	printf("calib %zu %lu %lu %lu %lu %lu\n", sizeof(pimax_cam_calib), O(pimax_cam_calib, cameras),
	       O(pimax_cam_calib, ombc), O(pimax_cam_calib, tbc), O(pimax_cam_calib, imu_cam_delta_s),
	       O(pimax_cam_calib, device_uid));
	printf("open_params %zu\n", sizeof(pimax_cam_open_params));
	if (argc < 3) {
		return 0;
	}

	pimax_cam_calib cal;
	char used[512];
	if (pimax_cam_calib_load(argv[1], &cal, used, sizeof(used)) != 0) {
		printf("calib error: %s\n", pimax_cam_last_error());
		return 1;
	}
	double r[9], t[3];
	if (pimax_cam_calib_stereo(&cal, 0, 1, r, t) != 0 || pimax_cam_calib_body_from_cam(&cal, 1, r, t) != 0) {
		printf("pose error: %s\n", pimax_cam_last_error());
		return 1;
	}
	printf("calib %u %s %s %.10f body1 %.4f %.4f %.4f\n", cal.camera_count, cal.device_uid, cal.cameras[1].name,
	       cal.cameras[1].intr.fx, t[0], t[1], t[2]);

	pimax_cam_open_params p = {0};
	p.verify_rows = 1;
	pimax_cam *c = pimax_cam_open(argv[2], &p);
	if (c == NULL) {
		printf("open error: %s\n", pimax_cam_last_error());
		return 1;
	}
	uint8_t *buf = malloc(700 * 480);
	pimax_cam_frame_info fi;
	int n = 0, r2;
	while ((r2 = pimax_cam_next(c, 100, &fi)) == 1) {
		unsigned long sum[4] = {0, 0, 0, 0};
		for (uint32_t tile = 0; tile < 4; tile++) {
			if (pimax_cam_copy_tile(c, tile, buf, 700) != 0) {
				printf("copy error: %s\n", pimax_cam_last_error());
				return 1;
			}
			for (int y = 0; y < 480; y++) {
				for (int x = 0; x < 640; x++) {
					sum[tile] += buf[y * 700 + x];
				}
			}
		}
		printf("frame %d ctr %u kind %u rows %u %u %u %u sums %lu %lu %lu %lu\n", n++, fi.counter, fi.kind,
		       fi.y_offset[0], fi.y_offset[1], fi.y_offset[2], fi.y_offset[3], sum[0], sum[1], sum[2], sum[3]);
	}
	pimax_cam_stats st;
	pimax_cam_get_stats(c, &st);
	printf("end %d: %s; frames %lu tracking %lu led %lu mismatch %lu\n", r2, pimax_cam_last_error(),
	       (unsigned long)st.frames, (unsigned long)st.tracking, (unsigned long)st.led,
	       (unsigned long)st.row_mismatch);
	if (pimax_cam_copy_tile(c, 9, buf, 700) != -1) {
		return 1;
	}
	pimax_cam_close(c);
	free(buf);
	return 0;
}
