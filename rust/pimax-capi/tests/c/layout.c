/* Prints sizeof/offsetof of every public struct of the REAL libpimax/include/pimax.h. */
#include <stddef.h>
#include <stdio.h>
#include "pimax.h"
#define F(s, f) printf(" %s=%zu", #f, offsetof(s, f))
int main(void)
{
    printf("imu size=%zu align=%zu", sizeof(pimax_imu_sample), _Alignof(pimax_imu_sample));
    F(pimax_imu_sample, ts_us); F(pimax_imu_sample, raw_accel); F(pimax_imu_sample, raw_gyro); F(pimax_imu_sample, accel_g);
    F(pimax_imu_sample, gyro_dps); F(pimax_imu_sample, ipd_mm); F(pimax_imu_sample, sample_in_report); printf("\n");
    printf("ctrl size=%zu align=%zu", sizeof(pimax_controller_report), _Alignof(pimax_controller_report));
    F(pimax_controller_report, slot); F(pimax_controller_report, ts_us); F(pimax_controller_report, stick_x);
    F(pimax_controller_report, stick_y); F(pimax_controller_report, trigger); F(pimax_controller_report, grip);
    F(pimax_controller_report, battery); F(pimax_controller_report, buttons); F(pimax_controller_report, touch);
    F(pimax_controller_report, raw_accel); F(pimax_controller_report, raw_gyro); F(pimax_controller_report, raw); printf("\n");
    printf("status size=%zu align=%zu", sizeof(pimax_controller_status), _Alignof(pimax_controller_status));
    F(pimax_controller_status, slot); F(pimax_controller_status, battery); F(pimax_controller_status, fw); F(pimax_controller_status, raw); printf("\n");
    printf("callbacks size=%zu align=%zu", sizeof(pimax_callbacks), _Alignof(pimax_callbacks));
    F(pimax_callbacks, user); F(pimax_callbacks, on_state); F(pimax_callbacks, on_imu); F(pimax_callbacks, on_controller);
    F(pimax_callbacks, on_controller_status); F(pimax_callbacks, on_log); F(pimax_callbacks, on_reply); printf("\n");
    printf("state size=%zu DISCONNECTED=%d CONNECTED=%d\n", sizeof(pimax_state), PIMAX_DISCONNECTED, PIMAX_CONNECTED);
    printf("consts VID=%#x PID=%#x NUM_IF=%d PATH_MAX=%d REPORT=%d MAXDATA=%d L=%d R=%d B=%#x A=%#x STICK=%#x MENU=%#x TSTICK=%#x\n",
           PIMAX_VID, PIMAX_PID, PIMAX_NUM_IF, PIMAX_PATH_MAX, PIMAX_REPORT_SIZE, PIMAX_MAX_DATA, PIMAX_SLOT_LEFT, PIMAX_SLOT_RIGHT,
           PIMAX_BTN_B, PIMAX_BTN_A, PIMAX_BTN_STICK, PIMAX_BTN_MENU, PIMAX_TOUCH_STICK);
    return 0;
}
