// In-process test of the REAL driver glue (pimax_ctrl.c as built into monado-service): synthetic controller reports go in
// through pimax_ctrl_on_report(), joints come out of xdev->get_hand_tracking(). No service, no headset, no GPU.
// Expected numbers are derived independently of the Rust mapping (flexion = 3.6296 * curl for the index finger, 3.861 *
// curl for the others, |10deg - 1.57 curl| + |10deg - 1.4 curl| for the thumb, from the Monado simulation).
// Built and run by tests/driver_glue.rs (cargo test -p pimax-hands --test driver_glue -- --ignored) against a configured
// and built Monado tree that contains the hands patch; prints PASS.
#include <math.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include "xrt/xrt_device.h"
#include "util/u_logging.h"
#include "os/os_time.h"
#include "pimax.h"
#include "pimax_ctrl.h"

static int fails;
#define CHECK(c, ...) do { if (!(c)) { fails++; printf("FAIL line %d: ", __LINE__); printf(__VA_ARGS__); printf("\n"); } } while (0)

static double ang(const struct xrt_vec3 *a, const struct xrt_vec3 *b)
{
	double d = a->x * b->x + a->y * b->y + a->z * b->z;
	double na = sqrt(a->x * a->x + a->y * a->y + a->z * a->z), nb = sqrt(b->x * b->x + b->y * b->y + b->z * b->z);
	double c = d / (na * nb);
	return acos(c > 1 ? 1 : (c < -1 ? -1 : c));
}

// sum of the bend angles of a finger given its joint indices
static double flex(const struct xrt_hand_joint_set *s, const int *idx, int n)
{
	struct xrt_vec3 bones[5];
	for (int i = 0; i + 1 < n; i++) {
		const struct xrt_vec3 *p0 = &s->values.hand_joint_set_default[idx[i]].relation.pose.position;
		const struct xrt_vec3 *p1 = &s->values.hand_joint_set_default[idx[i + 1]].relation.pose.position;
		bones[i] = (struct xrt_vec3){p1->x - p0->x, p1->y - p0->y, p1->z - p0->z};
	}
	double sum = 0;
	for (int i = 0; i + 2 < n; i++) sum += ang(&bones[i], &bones[i + 1]);
	return sum;
}

static const int THUMB[] = {2, 3, 4, 5}, INDEX[] = {6, 7, 8, 9, 10}, MIDDLE[] = {11, 12, 13, 14, 15}, RING[] = {16, 17, 18, 19, 20},
                 LITTLE[] = {21, 22, 23, 24, 25};

static void feed(struct pimax_ctrl_ctx *ctx, int slot, int ms, uint8_t trigger, uint8_t grip, uint32_t buttons, uint8_t touch, int sx, int sy)
{
	pimax_controller_report r;
	memset(&r, 0, sizeof r);
	r.slot = slot; r.trigger = trigger; r.grip = grip; r.buttons = buttons; r.touch = touch; r.stick_x = sx; r.stick_y = sy;
	for (int t = 0; t < ms; t += 4) {
		pimax_ctrl_on_report(ctx, &r);
		usleep(4000);
	}
}

static struct xrt_hand_joint_set get(struct xrt_device *d, enum xrt_input_name n)
{
	struct xrt_hand_joint_set s;
	memset(&s, 0, sizeof s);
	int64_t ts = 0;
	xrt_result_t xr = d->get_hand_tracking(d, n, (int64_t)os_monotonic_get_ns(), &s, &ts);
	CHECK(xr == XRT_SUCCESS, "get_hand_tracking -> %d", xr);
	return s;
}

int main(void)
{
	struct xrt_device *devs[2] = {NULL, NULL};
	struct pimax_ctrl_ctx *ctx = pimax_ctrl_create(NULL, U_LOGGING_INFO, devs);
	CHECK(ctx != NULL && devs[0] && devs[1], "create");
	CHECK(devs[0]->supported.hand_tracking && devs[1]->supported.hand_tracking, "supported.hand_tracking");
	int found_l = 0, found_r = 0;
	for (uint32_t i = 0; i < devs[0]->input_count; i++) found_l |= devs[0]->inputs[i].name == XRT_INPUT_HT_CONFORMING_LEFT;
	for (uint32_t i = 0; i < devs[1]->input_count; i++) found_r |= devs[1]->inputs[i].name == XRT_INPUT_HT_CONFORMING_RIGHT;
	CHECK(found_l && found_r, "HT inputs %d %d", found_l, found_r);

	// before any report: inactive
	struct xrt_hand_joint_set s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	CHECK(!s.is_active, "active without reports");

	// wrong input name is rejected
	{
		struct xrt_hand_joint_set x; int64_t ts;
		xrt_result_t xr = devs[0]->get_hand_tracking(devs[0], XRT_INPUT_HT_CONFORMING_RIGHT, 0, &x, &ts);
		CHECK(xr == XRT_ERROR_INPUT_UNSUPPORTED, "wrong side accepted (%d)", xr);
	}

	// S1: left, trigger fully pulled
	feed(ctx, PIMAX_SLOT_LEFT, 1200, 255, 0, 0, 0, 0, 0);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	double fi = flex(&s, INDEX, 5), fm = flex(&s, MIDDLE, 5), fr = flex(&s, RING, 5), fl = flex(&s, LITTLE, 5), ft = flex(&s, THUMB, 4);
	printf("S1 trigger: index %.3f middle %.3f ring %.3f little %.3f thumb %.3f active %d\n", fi, fm, fr, fl, ft, s.is_active);
	CHECK(s.is_active, "S1 inactive");
	CHECK(fabs(fi - 3.6296 * 0.85) < 0.05, "S1 index flexion %f", fi);
	CHECK(fabs(fm - 3.861 * 0.25) < 0.05 && fabs(fr - 3.861 * 0.25) < 0.05 && fabs(fl - 3.861 * 0.25) < 0.05, "S1 lower fingers %f %f %f", fm, fr, fl);
	CHECK(fabs(ft - 0.349) < 0.05, "S1 thumb %f", ft);

	// pose: wrist = grip * offset; with no head the fixed grip pose is (-0.20,-0.30,-0.35), identity rotation
	{
		struct xrt_space_relation grip;
		devs[0]->get_tracked_pose(devs[0], XRT_INPUT_TOUCH_GRIP_POSE, (int64_t)os_monotonic_get_ns(), &grip);
		printf("S1 pose: grip (%.3f %.3f %.3f) hand_pose (%.4f %.4f %.4f) flags grip 0x%x hand 0x%x\n", grip.pose.position.x, grip.pose.position.y,
		       grip.pose.position.z, s.hand_pose.pose.position.x, s.hand_pose.pose.position.y, s.hand_pose.pose.position.z,
		       (unsigned)grip.relation_flags, (unsigned)s.hand_pose.relation_flags);
		CHECK(fabs(s.hand_pose.pose.position.x - (grip.pose.position.x - 0.0289)) < 0.001 &&
		          fabs(s.hand_pose.pose.position.y - (grip.pose.position.y + 0.0629)) < 0.001 &&
		          fabs(s.hand_pose.pose.position.z - (grip.pose.position.z + 0.0146)) < 0.001,
		      "left wrist is not grip + (-0.0289, 0.0629, 0.0146)");
		CHECK(s.hand_pose.relation_flags == grip.relation_flags, "hand_pose flags 0x%x != grip flags 0x%x", (unsigned)s.hand_pose.relation_flags, (unsigned)grip.relation_flags);
	}

	// S2: left, grip fully squeezed, trigger released
	feed(ctx, PIMAX_SLOT_LEFT, 1200, 0, 255, 0, 0, 0, 0);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	fi = flex(&s, INDEX, 5); fm = flex(&s, MIDDLE, 5); fr = flex(&s, RING, 5); fl = flex(&s, LITTLE, 5);
	printf("S2 grip: index %.3f middle %.3f ring %.3f little %.3f\n", fi, fm, fr, fl);
	CHECK(fabs(fi - 3.6296 * 0.10) < 0.05, "S2 index %f", fi);
	CHECK(fabs(fm - 3.861) < 0.06 && fabs(fr - 3.861) < 0.06 && fabs(fl - 3.861) < 0.06, "S2 lower fingers %f %f %f", fm, fr, fl);

	// S3: left, thumb touches A/X (touch bit 0): thumb curl 0.45 -> flexion |0.1745-0.7065|+|0.1745-0.63| = 0.9875
	feed(ctx, PIMAX_SLOT_LEFT, 1000, 0, 0, 0, 0x01, 0, 0);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	ft = flex(&s, THUMB, 4);
	printf("S3 touch X: thumb %.3f\n", ft);
	CHECK(fabs(ft - 0.9875) < 0.06, "S3 thumb %f", ft);

	// S4: right controller (slot 3), B pressed with touch B: thumb curl 0.65 -> flexion 1.5815; left stays as it was
	feed(ctx, PIMAX_SLOT_RIGHT, 1000, 0, 0, PIMAX_BTN_B, 0x02, 0, 0);
	struct xrt_hand_joint_set sr = get(devs[1], XRT_INPUT_HT_CONFORMING_RIGHT);
	ft = flex(&sr, THUMB, 4);
	double ftl = flex(&s, THUMB, 4);
	printf("S4 right press B: thumb %.3f (left thumb earlier %.3f), right index %.3f\n", ft, ftl, flex(&sr, INDEX, 5));
	CHECK(sr.is_active, "S4 right inactive");
	CHECK(fabs(ft - 1.5815) < 0.07, "S4 thumb %f", ft);
	CHECK(fabs(flex(&sr, MIDDLE, 5) - 3.861 * 0.25) < 0.05, "S4 right lower fingers moved");
	{
		struct xrt_space_relation grip;
		devs[1]->get_tracked_pose(devs[1], XRT_INPUT_TOUCH_GRIP_POSE, (int64_t)os_monotonic_get_ns(), &grip);
		CHECK(fabs(sr.hand_pose.pose.position.x - (grip.pose.position.x + 0.0289)) < 0.001, "right wrist x");
	}

	// S5: stick sweep: deflection puts the thumb on the stick at once
	feed(ctx, PIMAX_SLOT_LEFT, 300, 0, 0, 0, 0, 100, 0);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	ft = flex(&s, THUMB, 4);
	printf("S5 stick deflected: thumb %.3f (stick 0.55 -> %.3f)\n", ft, fabs(0.1745 - 1.57 * 0.55) + fabs(0.1745 - 1.4 * 0.55));
	CHECK(fabs(ft - (fabs(0.1745 - 1.57 * 0.55) + fabs(0.1745 - 1.4 * 0.55))) < 0.1, "S5 thumb %f", ft);

	// S6: silence for 1.7 s -> inactive (reports stopped)
	usleep(1700000);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	CHECK(!s.is_active, "still active after 1.7 s of silence");
	// and it comes back with the next report, filters restarted from it
	feed(ctx, PIMAX_SLOT_LEFT, 100, 255, 255, 0, 0, 0, 0);
	s = get(devs[0], XRT_INPUT_HT_CONFORMING_LEFT);
	printf("S6 wake: active %d index %.3f middle %.3f\n", s.is_active, flex(&s, INDEX, 5), flex(&s, MIDDLE, 5));
	CHECK(s.is_active && fabs(flex(&s, INDEX, 5) - 3.6296 * 0.85) < 0.1 && fabs(flex(&s, MIDDLE, 5) - 3.861) < 0.1, "S6 restart from the first report");

	for (int i = 0; i < 2; i++) xrt_device_destroy(&devs[i]);
	pimax_ctrl_stop(ctx);   // drops the HMD reference (third ref); the controllers released theirs above
	if (fails == 0) printf("PASS\n");
	return fails != 0;
}
