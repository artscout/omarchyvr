/* Reads 64-byte records from stdin and prints what the public decoders return. Built twice: against the original C
 * decode.c and against the Rust libpimax_capi; the two outputs must be identical. */
#include <stdio.h>
#include <string.h>
#include "pimax.h"
int main(void)
{
    uint8_t r[64];
    unsigned long n = 0;
    while (fread(r, 1, 64, stdin) == 64) {
        pimax_imu_sample s[2];
        pimax_controller_report c;
        memset(s, 0, sizeof s);
        memset(&c, 0, sizeof c);
        int i = pimax_decode_imu(r, s);
        int k = pimax_decode_controller(r, &c);
        if (i) {
            for (int j = 0; j < 2; j++)
                printf("%lu I %u %d %d,%d,%d %d,%d,%d %a,%a,%a %a,%a,%a %a\n", n, s[j].ts_us, s[j].sample_in_report, s[j].raw_accel[0],
                       s[j].raw_accel[1], s[j].raw_accel[2], s[j].raw_gyro[0], s[j].raw_gyro[1], s[j].raw_gyro[2], s[j].accel_g[0],
                       s[j].accel_g[1], s[j].accel_g[2], s[j].gyro_dps[0], s[j].gyro_dps[1], s[j].gyro_dps[2], s[j].ipd_mm);
        }
        if (k) /* battery masked: the Rust library reports percent (byte46 & 0x7f), the C one the raw byte */
            printf("%lu C %u %u %d %d %u %u %u %u %u %d,%d,%d %d,%d,%d raw46=%u rawok=%d\n", n, c.slot, c.ts_us, c.stick_x, c.stick_y,
                   c.trigger, c.grip, c.battery & 0x7f, c.buttons, c.touch, c.raw_accel[0], c.raw_accel[1], c.raw_accel[2],
                   c.raw_gyro[0], c.raw_gyro[1], c.raw_gyro[2], c.raw[46], memcmp(c.raw, r, 64) == 0);
        if (!i && !k) printf("%lu -\n", n);
        n++;
    }
    return 0;
}
