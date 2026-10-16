/* Full-ABI session test written in C against the REAL pimax.h: fake headset over seqpacket socketpairs. */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#include "pimax.h"

pimax_t *pimax_open_with_fds(const pimax_callbacks *cb, const int read_fds[4], int ctl_write_fd); /* extension, not in pimax.h */

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #c); exit(1); } } while (0)

static int n_state, last_state = -1, n_imu, n_ctrl, n_status, n_log, n_reply;
static pimax_imu_sample imu[4];
static pimax_controller_report ctrl;
static pimax_controller_status status[4];
static char logline[600];
static int tag;
#define A(x) __atomic_load_n(&(x), __ATOMIC_ACQUIRE)
#define INC(x) __atomic_add_fetch(&(x), 1, __ATOMIC_RELEASE)

static void on_state(void *u, pimax_state s) { CHECK(u == &tag); last_state = s; INC(n_state); }
static void on_imu(void *u, const pimax_imu_sample *s) { CHECK(u == &tag); if (n_imu < 4) imu[n_imu] = *s; INC(n_imu); }
static void on_ctrl(void *u, const pimax_controller_report *r) { CHECK(u == &tag); ctrl = *r; INC(n_ctrl); }
static void on_status(void *u, const pimax_controller_status *s) { CHECK(u == &tag); if (n_status < 4) status[n_status] = *s; INC(n_status); }
static void on_log(void *u, const char *l) { CHECK(u == &tag); strncpy(logline, l, sizeof logline - 1); INC(n_log); }
static void on_reply(void *u, const uint8_t r[PIMAX_REPORT_SIZE]) { CHECK(u == &tag); CHECK(r[0] == 0x4c || r[0] != 0); INC(n_reply); }

static void msleep(int ms) { struct timespec t = { ms / 1000, (ms % 1000) * 1000000L }; nanosleep(&t, NULL); }

static int dev_if[4], host_if[4], ctl_dev, ctl_host;
static int stop_responder;

static int recv_cmd(uint8_t out[64], int timeout_ms)
{
    for (int i = 0; i < timeout_ms; i++) {
        uint8_t b[65];
        ssize_t n = recv(ctl_dev, b, sizeof b, MSG_DONTWAIT);
        if (n == 65) { memcpy(out, b + 1, 64); CHECK(b[0] == 0); return 1; }
        msleep(1);
    }
    return 0;
}

static void *responder(void *arg)
{
    (void)arg;
    while (!A(stop_responder)) {
        uint8_t c[64], r[64] = { 0 };
        if (!recv_cmd(c, 20)) continue;
        if (c[2] == 0x16 && c[3] == 0) {
            memcpy(r, "\x4c\x1a\x16\x00\x05H1234", 10);
            send(dev_if[0], r, 64, 0);
        } else if (c[2] == 0x03) {
            memcpy(r, "\x4c\x07\x03\x00\x00\x08\x2d\x6b", 8);
            r[3] = c[3];
            send(dev_if[0], r, 64, 0);
        }
    }
    return NULL;
}

#define wait_for(vp, want) do { for (int i_ = 0; i_ < 2000 && A(*(vp)) < (want); i_++) msleep(1); CHECK(A(*(vp)) >= (want)); } while (0)

int main(void)
{
    for (int i = 0; i < 4; i++) {
        int sv[2];
        CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) == 0);
        host_if[i] = sv[0]; dev_if[i] = sv[1];
    }
    int cv[2];
    CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, cv) == 0);
    ctl_host = cv[0]; ctl_dev = cv[1];

    pimax_callbacks cb = { .user = &tag, .on_state = on_state, .on_imu = on_imu, .on_controller = on_ctrl,
                           .on_controller_status = on_status, .on_log = on_log, .on_reply = on_reply };
    pimax_t *h = pimax_open_with_fds(&cb, host_if, ctl_host);
    CHECK(h);
    CHECK(pimax_wait_connected(h, 3000) == 0);
    CHECK(pimax_is_connected(h) == 1);
    CHECK(A(last_state) == PIMAX_CONNECTED);

    uint8_t c[64];
    CHECK(recv_cmd(c, 1000));
    CHECK(!memcmp(c, "\x58\x06\x1a\x05\x01\x00\x00", 7));            /* wake */
    pimax_set_video_rate(h, 72);
    int saw72 = 0;
    for (int i = 0; i < 6 && !saw72; i++) { if (recv_cmd(c, 400) && c[2] == 0x39 && c[4] == 72) saw72 = 1; }
    CHECK(saw72);

    /* IMU: ts=5_000_000, sample 0: acc (2048,-1024,0) gyro (164,0,-164), ipd 65.90 */
    uint8_t r[64] = { 0x53, 0x31, 0x08, 0x00 };
    uint32_t ts = 5000000;
    memcpy(r + 4, &ts, 4);
    r[14] = 0x08; r[15] = 0x00; r[16] = 0xfc; r[17] = 0x00; r[20] = 0x00; r[21] = 0xa4; r[24] = 0xff; r[25] = 0x5c;
    r[45] = 6590 & 255; r[46] = 6590 >> 8;
    CHECK(send(dev_if[2], r, 64, 0) == 64);
    wait_for(&n_imu, 2);
    CHECK(imu[0].sample_in_report == 0 && imu[1].sample_in_report == 1);
    CHECK(imu[0].ts_us == 5000000 && imu[1].ts_us == 5001000);
    CHECK(imu[0].raw_accel[0] == 2048 && imu[0].accel_g[0] == 1.0f && imu[0].accel_g[1] == -0.5f);
    CHECK(imu[0].raw_gyro[0] == 164 && imu[0].gyro_dps[0] == 10.0f && imu[0].raw_gyro[2] == -164);
    CHECK(imu[0].ipd_mm > 65.89f && imu[0].ipd_mm < 65.91f);

    /* controller on IF1 */
    uint8_t cr[64] = { 0x53, 0x32, 0x08, 0x03, 0x43, 255, 1, 200 };
    cr[44] = 99; cr[46] = 0xda; cr[47] = 0x0a; cr[48] = 1; cr[49] = 0x08;
    CHECK(send(dev_if[1], cr, 64, 0) == 64);
    wait_for(&n_ctrl, 1);
    CHECK(ctrl.slot == PIMAX_SLOT_RIGHT && ctrl.stick_x == 127 && ctrl.stick_y == -127 && ctrl.trigger == 200 && ctrl.grip == 99);
    CHECK(ctrl.battery == 90 && ctrl.raw[46] == 0xda);
    CHECK(ctrl.buttons == (PIMAX_BTN_A | PIMAX_BTN_STICK | PIMAX_BTN_MENU) && ctrl.touch == PIMAX_TOUCH_STICK);

    /* status heartbeat on IF0 */
    uint8_t st[64] = { 0x4c, 0x0f, 0x37, 0x02, 0x48, 1, 2, 3, 4, 0x49, 0, 2, 1, 0, 0x08, 0x2d };
    CHECK(send(dev_if[0], st, 64, 0) == 64);
    wait_for(&n_status, 1);
    CHECK(status[0].slot == 2 && status[0].battery == 0x49 && status[0].fw[0] == 0x08 && status[0].fw[1] == 0x2d);
    CHECK(A(n_reply) >= 1);

    /* firmware log on IF3 */
    uint8_t lg[64] = "hello fw\r\n";
    CHECK(send(dev_if[3], lg, 64, 0) == 64);
    wait_for(&n_log, 1);
    CHECK(!strcmp(logline, "hello fw"));

    /* requests */
    pthread_t th;
    pthread_create(&th, NULL, responder, NULL);
    char ser[32];
    CHECK(pimax_read_serial(h, 0, ser, sizeof ser, 1000) == 5 && !strcmp(ser, "H1234"));
    CHECK(pimax_read_serial(h, 0, ser, 4, 1000) == 3 && !strcmp(ser, "H12"));   /* truncated to cap-1, still NUL terminated */
    uint8_t ver[64];
    CHECK(pimax_read_version(h, 2, ver, sizeof ver, 1000) == 4 && ver[1] == 0x08 && ver[3] == 0x6b);
    uint8_t rep[64];
    int n = pimax_request(h, 0x16, 0, (const uint8_t[]){ 0x1e }, 1, rep, sizeof rep, 1000);
    CHECK(n == 23 && rep[0] == 5 && !memcmp(rep + 1, "H1234", 5));
    CHECK(pimax_request(h, 0x2c, 0, NULL, 0, rep, sizeof rep, 120) == -ETIMEDOUT);
    uint8_t big[61] = { 0 };
    CHECK(pimax_send(h, 0x16, 0, big, sizeof big) == -EMSGSIZE);
    CHECK(pimax_send(h, 0x16, 0, big, 60) == 0);
    __atomic_store_n(&stop_responder, 1, __ATOMIC_RELEASE);
    pthread_join(th, NULL);

    /* attach burst and video prepare wire forms */
    while (recv_cmd(c, 30)) { }
    CHECK(pimax_controller_attach(h, PIMAX_SLOT_LEFT) == 0);
    uint8_t want[4][5] = { { 0x58, 0x04, 0x16, 2, 0x1e }, { 0x58, 0x03, 0x50, 2, 0 }, { 0x58, 0x07, 0x0a, 2, 0 }, { 0x58, 0x03, 0x03, 2, 0 } };
    for (int i = 0; i < 4; i++) {
        int ok = 0;
        for (int t = 0; t < 20 && !ok; t++) { CHECK(recv_cmd(c, 200)); if (c[2] != 0x39) ok = 1; }
        CHECK(!memcmp(c, want[i], 5));
    }
    CHECK(pimax_video_prepare(h, 72) == 0);
    uint8_t vp[4][6] = { { 0x58, 0x05, 0x5b, 0x05, 2, 2 }, { 0x58, 0x04, 0x29, 0x00, 1, 0 }, { 0x58, 0x0d, 0x1a, 0x05, 2, 72 }, { 0x58, 0x0d, 0x1a, 0x05, 5, 0 } };
    for (int i = 0; i < 4; i++) {
        int ok = 0;
        for (int t = 0; t < 20 && !ok; t++) { CHECK(recv_cmd(c, 200)); if (c[2] != 0x39) ok = 1; }
        CHECK(!memcmp(c, vp[i], 6));
    }

    /* unplug: all device ends closed */
    for (int i = 0; i < 4; i++) close(dev_if[i]);
    close(ctl_dev);
    for (int i = 0; i < 2000 && A(last_state) == PIMAX_CONNECTED; i++) msleep(1);
    CHECK(A(last_state) == PIMAX_DISCONNECTED && pimax_is_connected(h) == 0);
    CHECK(pimax_send(h, 3, 0, NULL, 0) == -ENOTCONN);
    pimax_close(h);
    printf("OK\n");
    return 0;
}
