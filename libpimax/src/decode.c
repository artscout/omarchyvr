#include <string.h>
#include "pimax.h"

static int16_t be16(const uint8_t *p) { return (int16_t)(((unsigned)p[0] << 8) | p[1]); }
static uint32_t le32(const uint8_t *p) { return p[0] | (p[1] << 8) | (p[2] << 16) | ((uint32_t)p[3] << 24); }

/* Headset IMU: 53 31 08 00 | ts u32 LE @4 | samples @12 and @28 (u16 BE temp, 3x BE accel, 3x BE gyro) | ipd u16 LE @45 */
int pimax_decode_imu(const uint8_t r[PIMAX_REPORT_SIZE], pimax_imu_sample out[2])
{
    if (r[0] != 0x53 || r[1] != 0x31)
        return 0;
    uint32_t ts = le32(r + 4);
    float ipd = (float)(r[45] | (r[46] << 8)) / 100.0f;
    for (int i = 0; i < 2; i++) {
        const uint8_t *s = r + 12 + 16 * i + 2;
        pimax_imu_sample *o = &out[i];
        for (int k = 0; k < 3; k++) {
            o->raw_accel[k] = be16(s + 2 * k);
            o->raw_gyro[k] = be16(s + 6 + 2 * k);
            o->accel_g[k] = (float)o->raw_accel[k] / 2048.0f;
            o->gyro_dps[k] = (float)o->raw_gyro[k] / 16.4f;
        }
        o->ts_us = ts + 1000u * (unsigned)i;
        o->ipd_mm = ipd;
        o->sample_in_report = i;
    }
    return 1;
}

/* Controller: 53 32 08 SS 43 | sx sy trig | ts u32 LE @8 | s0 @16 | s1 @30 | grip @44 | batt @46 | btn @47,48 | touch @49 */
int pimax_decode_controller(const uint8_t r[PIMAX_REPORT_SIZE], pimax_controller_report *o)
{
    if (r[0] != 0x53 || r[1] != 0x32 || r[2] != 0x08)
        return 0;
    if (r[3] != PIMAX_SLOT_LEFT && r[3] != PIMAX_SLOT_RIGHT)
        return 0;
    memset(o, 0, sizeof *o);
    memcpy(o->raw, r, PIMAX_REPORT_SIZE);
    o->slot = r[3];
    o->stick_x = (int)r[5] - 128;
    o->stick_y = (int)r[6] - 128;
    o->trigger = r[7];
    o->ts_us = le32(r + 8);
    const uint8_t *s0 = r + 16, *s1 = r + 30;
    int any0 = 0;
    for (int i = 0; i < 12; i++) any0 |= s0[i];
    const uint8_t *s = any0 ? s0 : s1;      /* second sample is zero when the packet holds one */
    for (int k = 0; k < 3; k++) {
        o->raw_accel[k] = be16(s + 2 * k);
        o->raw_gyro[k] = be16(s + 6 + 2 * k);
    }
    o->grip = r[44];
    o->battery = r[46];
    o->buttons = (r[47] & (PIMAX_BTN_B | PIMAX_BTN_A | PIMAX_BTN_STICK)) | ((r[48] & 1) ? PIMAX_BTN_MENU : 0);
    o->touch = r[49];
    return 1;
}
