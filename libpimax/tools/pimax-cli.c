/* pimax-cli: imu | log | controllers | info   [seconds]
 * Deliberately has no video/panel commands. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include "pimax.h"

static pimax_imu_sample last_imu;
static unsigned long imu_n, ctrl_n[4], st_n;
static int want_log, want_ctrl;

static void on_state(void *u, pimax_state s) { (void)u; printf("state: %s\n", s ? "connected" : "disconnected"); fflush(stdout); }
static void on_imu(void *u, const pimax_imu_sample *s) { (void)u; last_imu = *s; imu_n++; }
static void on_log(void *u, const char *l) { (void)u; if (want_log) { printf("fwlog: %s\n", l); fflush(stdout); } }
static void on_ctrl(void *u, const pimax_controller_report *r)
{
    (void)u;
    ctrl_n[r->slot & 3]++;
    static unsigned div[4];
    if (want_ctrl && (div[r->slot & 3]++ % 25) == 0)
        printf("ctrl %c stick=(%+4d,%+4d) trig=%3u grip=%3u bat=%u%% btn=0x%02x touch=0x%02x acc=(%d,%d,%d) n=%lu\n",
               r->slot == PIMAX_SLOT_LEFT ? 'L' : 'R', r->stick_x, r->stick_y, r->trigger, r->grip, r->battery,
               r->buttons, r->touch, r->raw_accel[0], r->raw_accel[1], r->raw_accel[2], ctrl_n[r->slot & 3]);
}
static void on_status(void *u, const pimax_controller_status *s)
{
    (void)u; st_n++;
    if (want_ctrl)
        printf("status 0x37 slot %u battery=%u%% fw=%02x%02x\n", s->slot, s->battery, s->fw[0], s->fw[1]);
}

static void hex(const uint8_t *p, int n) { for (int i = 0; i < n; i++) printf("%02x%s", p[i], i + 1 < n ? " " : ""); }
static void msleep(int ms) { struct timespec t = { ms / 1000, (ms % 1000) * 1000000L }; nanosleep(&t, NULL); }

int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "usage: %s imu|log|controllers|info [seconds]\n", argv[0]); return 2; }
    const char *sub = argv[1];
    double secs = argc > 2 ? atof(argv[2]) : 5.0;
    int is_imu = !strcmp(sub, "imu"), is_log = !strcmp(sub, "log"), is_ctrl = !strcmp(sub, "controllers"), is_info = !strcmp(sub, "info");
    if (!is_imu && !is_log && !is_ctrl && !is_info) { fprintf(stderr, "unknown subcommand %s\n", sub); return 2; }
    want_log = is_log; want_ctrl = is_ctrl;

    pimax_callbacks cb = { .on_state = on_state, .on_imu = on_imu, .on_log = on_log,
                           .on_controller = on_ctrl, .on_controller_status = on_status };
    pimax_t *h = pimax_open(&cb);
    if (!h) { perror("pimax_open"); return 1; }
    if (pimax_wait_connected(h, 10000)) { fprintf(stderr, "headset not found (34a4:0044)\n"); pimax_close(h); return 1; }

    int rc = 0;
    if (is_info) {
        char ser[64]; uint8_t v[PIMAX_REPORT_SIZE];
        int n = pimax_read_serial(h, 0, ser, sizeof ser, 1000);
        if (n < 0) { printf("serial: error %d (%s)\n", n, strerror(-n)); rc = 1; } else printf("serial: %s\n", ser);
        n = pimax_read_version(h, 0, v, sizeof v, 1000);
        if (n < 0) { printf("version: error %d (%s)\n", n, strerror(-n)); rc = 1; } else { printf("version: "); hex(v, n); printf("\n"); }
        pimax_close(h);
        return rc;
    }
    if (is_ctrl) { pimax_controller_attach(h, PIMAX_SLOT_LEFT); pimax_controller_attach(h, PIMAX_SLOT_RIGHT); }

    struct timespec t0, t;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    unsigned long prev = 0;
    for (int sec = 1; ; sec++) {
        msleep(1000);
        clock_gettime(CLOCK_MONOTONIC, &t);
        double el = (t.tv_sec - t0.tv_sec) + (t.tv_nsec - t0.tv_nsec) / 1e9;
        if (is_imu) {
            const pimax_imu_sample *s = &last_imu;
            printf("imu %.0f samples/s (total %lu) | a=(%.3f,%.3f,%.3f)g |a|=%.3f gyro=(%.2f,%.2f,%.2f)dps ts=%u ipd=%.2fmm\n",
                   (double)(imu_n - prev), imu_n, s->accel_g[0], s->accel_g[1], s->accel_g[2],
                   __builtin_sqrtf(s->accel_g[0]*s->accel_g[0] + s->accel_g[1]*s->accel_g[1] + s->accel_g[2]*s->accel_g[2]),
                   s->gyro_dps[0], s->gyro_dps[1], s->gyro_dps[2], s->ts_us, s->ipd_mm);
            prev = imu_n;
        } else if (is_ctrl && sec % 5 == 0) {
            printf("[%.0fs] reports L=%lu R=%lu status=%lu\n", el, ctrl_n[2], ctrl_n[3], st_n);
        }
        fflush(stdout);
        if (el >= secs) break;
    }
    if (is_imu) printf("done: %lu IMU samples in %.1fs\n", imu_n, secs);
    if (is_ctrl) printf("done: reports L=%lu R=%lu status=%lu\n", ctrl_n[2], ctrl_n[3], st_n);
    pimax_close(h);
    return 0;
}
