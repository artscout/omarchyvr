/* A C program using the library exactly like the Monado driver does: link libpimax_hands.a, drive a hand, read it back.
 * Prints PASS and exits 0, or prints what failed. */
#include <math.h>
#include <stdio.h>
#include <string.h>
#include "pimax_hands.h"

static int fails;
#define CHECK(c, ...) do { if (!(c)) { fails++; printf("FAIL line %d: ", __LINE__); printf(__VA_ARGS__); printf("\n"); } } while (0)

int main(void)
{
	CHECK(pimax_hands_abi_version() == PIMAX_HANDS_ABI_VERSION, "abi %u", pimax_hands_abi_version());

	pimax_hands *h = pimax_hands_create();
	CHECK(h != NULL, "create");
	pimax_hands_out out;
	memset(&out, 0xff, sizeof out);
	pimax_hands_get(h, &out);
	CHECK(out.thumb_state == PIMAX_HANDS_THUMB_REST, "rest state %u", out.thumb_state);
	CHECK(out.curl[3] < 0.2f && out.curl[2] < 0.4f, "rest curls %f %f", out.curl[3], out.curl[2]);

	/* grip + trigger squeezed and thumb on the stick for 1.2 s of 250 Hz reports */
	pimax_hands_input in = {.trigger = 1.0f, .grip = 1.0f, .stick_x = 0.0f, .stick_y = 0.0f, .flags = PIMAX_HANDS_TOUCH_STICK};
	uint64_t t = 5000ull * 1000 * 1000;
	for (int i = 0; i < 300; i++) {
		t += 4ull * 1000 * 1000;
		pimax_hands_update(h, t, &in);
	}
	pimax_hands_get(h, &out);
	CHECK(out.thumb_state == PIMAX_HANDS_THUMB_STICK, "thumb state %u", out.thumb_state);
	CHECK(fabsf(out.curl[2] - 1.0f) < 0.01f, "middle %f", out.curl[2]);
	CHECK(out.curl[0] == out.curl[1] && out.curl[1] == out.curl[2], "lower fingers differ");
	CHECK(out.curl[3] > 0.7f && out.curl[3] < 0.9f, "index %f", out.curl[3]);
	CHECK(out.curl[4] > 0.4f && out.curl[4] < 0.7f, "thumb %f", out.curl[4]);
	CHECK(fabsf(out.index.joint_curls[1] - 1.43f * out.curl[3]) < 1e-3f, "index proximal %f", out.index.joint_curls[1]);
	CHECK(fabsf(out.middle.joint_curls[1] - 1.859f * out.curl[2]) < 1e-3f, "middle proximal %f", out.middle.joint_curls[1]);
	CHECK(out.thumb.splay == -0.35f, "thumb splay %f", out.thumb.splay);

	/* the raw report path: trigger 255, grip 255, touch bit 0 (thumb on A/X), A pressed */
	pimax_hands_reset(h);
	for (int i = 0; i < 300; i++) {
		t += 4ull * 1000 * 1000;
		pimax_hands_update_report(h, t, 255, 255, 0, 0, 0x02 /* PIMAX_BTN_A */, 0x01 /* touch A */);
	}
	pimax_hands_get(h, &out);
	CHECK(out.thumb_state == PIMAX_HANDS_THUMB_BTN1, "report path thumb state %u", out.thumb_state);
	CHECK(fabsf(out.curl[2] - 1.0f) < 0.01f && out.curl[3] > 0.8f, "report path curls %f %f", out.curl[2], out.curl[3]);
	CHECK(out.curl[4] > 0.55f, "A pressed: thumb deeper than a touch (%f)", out.curl[4]);

	pimax_hands_reset(h);
	pimax_hands_get(h, &out);
	CHECK(out.thumb_state == PIMAX_HANDS_THUMB_REST && out.curl[2] < 0.4f, "after reset");

	/* grip -> wrist: wrist about 6 cm up the grip +Y axis, mirror image for the other hand */
	pimax_hands_pose l, r;
	pimax_hands_grip_to_wrist(0, &l);
	pimax_hands_grip_to_wrist(1, &r);
	CHECK(l.position[1] > 0.05f && l.position[1] < 0.08f, "left y %f", l.position[1]);
	CHECK(fabsf(l.position[0] + r.position[0]) < 1e-6f && fabsf(l.position[1] - r.position[1]) < 1e-6f, "mirror");
	CHECK(fabsf(l.orientation[0] * l.orientation[0] + l.orientation[1] * l.orientation[1] + l.orientation[2] * l.orientation[2] +
	            l.orientation[3] * l.orientation[3] - 1.0f) < 1e-5f, "unit quaternion");

	pimax_hands_input d;
	pimax_hands_demo_input(2000ull * 1000 * 1000, 0, &d);
	CHECK(d.grip > 0.99f && d.trigger == 0.0f, "demo at 2 s: grip %f trigger %f", d.grip, d.trigger);

	/* NULL tolerance */
	pimax_hands_update_report(NULL, 1, 0, 0, 0, 0, 0, 0);
	pimax_hands_update(NULL, 1, &in);
	pimax_hands_update(h, 1, NULL);
	pimax_hands_get(NULL, &out);
	pimax_hands_get(h, NULL);
	pimax_hands_grip_to_wrist(0, NULL);
	pimax_hands_demo_input(0, 0, NULL);
	pimax_hands_reset(NULL);
	pimax_hands_destroy(NULL);

	pimax_hands_destroy(h);
	if (fails == 0) printf("PASS\n");
	return fails != 0;
}
