/* Prints sizeof/offsetof of every public struct of include/pimax_hands.h and the flag values. */
#include <stddef.h>
#include <stdio.h>
#include "pimax_hands.h"
#define F(s, f) printf(" %s=%zu", #f, offsetof(s, f))
int main(void)
{
	printf("input size=%zu align=%zu", sizeof(pimax_hands_input), _Alignof(pimax_hands_input));
	F(pimax_hands_input, trigger); F(pimax_hands_input, grip); F(pimax_hands_input, stick_x);
	F(pimax_hands_input, stick_y); F(pimax_hands_input, flags); printf("\n");
	printf("finger size=%zu align=%zu", sizeof(pimax_hands_finger), _Alignof(pimax_hands_finger));
	F(pimax_hands_finger, splay); F(pimax_hands_finger, joint_curls); printf("\n");
	printf("out size=%zu align=%zu", sizeof(pimax_hands_out), _Alignof(pimax_hands_out));
	F(pimax_hands_out, little); F(pimax_hands_out, ring); F(pimax_hands_out, middle); F(pimax_hands_out, index);
	F(pimax_hands_out, thumb); F(pimax_hands_out, curl); F(pimax_hands_out, thumb_state); printf("\n");
	printf("pose size=%zu align=%zu", sizeof(pimax_hands_pose), _Alignof(pimax_hands_pose));
	F(pimax_hands_pose, orientation); F(pimax_hands_pose, position); printf("\n");
	printf("consts abi=%u TB1=%#x TB2=%#x TS=%#x CB1=%#x CB2=%#x CS=%#x CM=%#x th=%u,%u,%u,%u,%u\n", PIMAX_HANDS_ABI_VERSION,
	       PIMAX_HANDS_TOUCH_BTN1, PIMAX_HANDS_TOUCH_BTN2, PIMAX_HANDS_TOUCH_STICK, PIMAX_HANDS_CLICK_BTN1,
	       PIMAX_HANDS_CLICK_BTN2, PIMAX_HANDS_CLICK_STICK, PIMAX_HANDS_CLICK_MENU, PIMAX_HANDS_THUMB_REST,
	       PIMAX_HANDS_THUMB_BTN1, PIMAX_HANDS_THUMB_BTN2, PIMAX_HANDS_THUMB_STICK, PIMAX_HANDS_THUMB_MENU);
	return 0;
}
