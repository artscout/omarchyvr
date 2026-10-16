#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include "pimax.h"

#define KEEPALIVE_NS 213000000LL
#define LOG_LINE_MAX 512

struct pimax {
    pimax_callbacks cb;
    pthread_t thread;
    atomic_bool stop;
    atomic_int connected;
    atomic_int rate;

    pthread_mutex_t lock;       /* protects ctl_fd, connected cond, pending request */
    pthread_cond_t cond;
    int ctl_fd;                 /* IF0, -1 when disconnected */

    pthread_mutex_t req_lock;   /* serialises pimax_request */
    int pend_active, pend_done;
    uint8_t pend_cmd, pend_opt;
    uint8_t pend_report[PIMAX_REPORT_SIZE];

    char logbuf[PIMAX_NUM_IF][LOG_LINE_MAX];
    size_t loglen[PIMAX_NUM_IF];
};

static int64_t now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void sleep_ms(int ms)
{
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000L };
    nanosleep(&ts, NULL);
}

/* ---- discovery ---- */

static int read_small(const char *path, char *buf, size_t cap)
{
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    size_t n = fread(buf, 1, cap - 1, f);
    fclose(f);
    buf[n] = 0;
    return (int)n;
}

int pimax_find_interfaces(char paths[PIMAX_NUM_IF][PIMAX_PATH_MAX])
{
    int mask = 0;
    DIR *d = opendir("/sys/class/hidraw");
    if (!d) return 0;
    struct dirent *e;
    while ((e = readdir(d))) {
        if (strncmp(e->d_name, "hidraw", 6)) continue;
        char p[PATH_MAX + 64], real[PATH_MAX], buf[1024];
        snprintf(p, sizeof p, "/sys/class/hidraw/%s/device/uevent", e->d_name);
        if (read_small(p, buf, sizeof buf) < 0) continue;
        if (!strstr(buf, "HID_ID=0003:000034A4:00000044")) continue;
        snprintf(p, sizeof p, "/sys/class/hidraw/%s/device", e->d_name);
        if (!realpath(p, real)) continue;
        char *slash = strrchr(real, '/');       /* parent = USB interface dir */
        if (!slash) continue;
        *slash = 0;
        snprintf(p, sizeof p, "%s/bInterfaceNumber", real);
        if (read_small(p, buf, sizeof buf) < 0) continue;
        long n = strtol(buf, NULL, 16);
        if (n < 0 || n >= PIMAX_NUM_IF) continue;
        snprintf(paths[n], PIMAX_PATH_MAX, "/dev/%.40s", e->d_name);
        mask |= 1 << n;
    }
    closedir(d);
    return mask;
}

/* ---- command building ---- */

static int build_cmd(uint8_t out[1 + PIMAX_REPORT_SIZE], uint8_t cmd, uint8_t opt, const void *data, size_t len)
{
    if (len > PIMAX_MAX_DATA) return -EMSGSIZE;
    memset(out, 0, 1 + PIMAX_REPORT_SIZE);      /* out[0] = report id 0 (unnumbered) */
    out[1] = 0x58;
    out[2] = (uint8_t)(len + 3);
    out[3] = cmd;
    out[4] = opt;
    if (len) memcpy(out + 5, data, len);
    return 0;
}

static int write_ctl(pimax_t *h, const uint8_t *buf)
{
    int rc = 0;
    pthread_mutex_lock(&h->lock);
    if (h->ctl_fd < 0) rc = -ENOTCONN;
    else if (write(h->ctl_fd, buf, 1 + PIMAX_REPORT_SIZE) < 0) rc = -errno;
    pthread_mutex_unlock(&h->lock);
    return rc;
}

int pimax_send(pimax_t *h, uint8_t cmd, uint8_t opt, const void *data, size_t len)
{
    uint8_t buf[1 + PIMAX_REPORT_SIZE];
    int rc = build_cmd(buf, cmd, opt, data, len);
    return rc ? rc : write_ctl(h, buf);
}

int pimax_request(pimax_t *h, uint8_t cmd, uint8_t opt, const void *data, size_t len,
                  uint8_t *reply, size_t reply_cap, int timeout_ms)
{
    uint8_t buf[1 + PIMAX_REPORT_SIZE];
    int rc = build_cmd(buf, cmd, opt, data, len);
    if (rc) return rc;

    pthread_mutex_lock(&h->req_lock);
    pthread_mutex_lock(&h->lock);
    h->pend_cmd = cmd; h->pend_opt = opt; h->pend_done = 0; h->pend_active = 1;
    pthread_mutex_unlock(&h->lock);

    rc = write_ctl(h, buf);
    pthread_mutex_lock(&h->lock);
    if (!rc) {
        struct timespec dl;
        clock_gettime(CLOCK_MONOTONIC, &dl);
        dl.tv_sec += timeout_ms / 1000;
        dl.tv_nsec += (long)(timeout_ms % 1000) * 1000000L;
        if (dl.tv_nsec >= 1000000000L) { dl.tv_sec++; dl.tv_nsec -= 1000000000L; }
        while (!h->pend_done) {
            if (pthread_cond_timedwait(&h->cond, &h->lock, &dl) == ETIMEDOUT) break;
        }
        if (h->pend_done) {
            /* 4c LL cmd opt data.. CK ; LL = datalen + 2 (incl. CK) + 1 */
            int dlen = (int)h->pend_report[1] - 3;
            if (dlen < 0) dlen = 0;
            if (dlen > PIMAX_REPORT_SIZE - 4) dlen = PIMAX_REPORT_SIZE - 4;
            if (reply && reply_cap) memcpy(reply, h->pend_report + 4, (size_t)dlen < reply_cap ? (size_t)dlen : reply_cap);
            rc = dlen;
        } else {
            rc = -ETIMEDOUT;
        }
    }
    h->pend_active = 0;
    pthread_mutex_unlock(&h->lock);
    pthread_mutex_unlock(&h->req_lock);
    return rc;
}

int pimax_read_serial(pimax_t *h, uint8_t opt, char *out, size_t cap, int timeout_ms)
{
    uint8_t r[PIMAX_REPORT_SIZE], arg = 0x1e;
    int n = pimax_request(h, 0x16, opt, &arg, 1, r, sizeof r, timeout_ms);
    if (n < 0) return n;
    /* data = <len> <ASCII serial> [tail] */
    size_t sl = n > 0 ? r[0] : 0;
    if (sl > (size_t)n - 1) sl = n > 0 ? (size_t)n - 1 : 0;
    if (sl >= cap) sl = cap ? cap - 1 : 0;
    if (cap) { memcpy(out, r + 1, sl); out[sl] = 0; }
    return (int)sl;
}

int pimax_read_version(pimax_t *h, uint8_t opt, uint8_t *out, size_t cap, int timeout_ms)
{
    return pimax_request(h, 0x03, opt, NULL, 0, out, cap, timeout_ms);
}

int pimax_controller_attach(pimax_t *h, uint8_t slot)
{
    static const uint8_t a16[] = { 0x1e }, a0a[] = { 0, 0, 0, 0x10 };
    int rc;
    if ((rc = pimax_send(h, 0x16, slot, a16, 1))) return rc;
    sleep_ms(10);
    if ((rc = pimax_send(h, 0x50, slot, NULL, 0))) return rc;
    sleep_ms(10);
    if ((rc = pimax_send(h, 0x0a, slot, a0a, 4))) return rc;
    sleep_ms(10);
    return pimax_send(h, 0x03, slot, NULL, 0);
}

int pimax_video_prepare(pimax_t *h, uint8_t fps)
{
    /* wire forms: 58 05 5b 05 02 02 / 58 04 29 00 01 / 58 0d 1a 05 02 FPS / 58 0d 1a 05 05 00 */
    static const uint8_t d5b[2] = { 0x02, 0x02 }, d29[1] = { 0x01 };
    uint8_t d1a[10] = { 0x02, fps };
    int rc;
    atomic_store(&h->rate, fps);
    if ((rc = pimax_send(h, 0x5b, 0x05, d5b, 2))) return rc;
    if ((rc = pimax_send(h, 0x29, 0x00, d29, 1))) return rc;
    if ((rc = pimax_send(h, 0x1a, 0x05, d1a, 10))) return rc;
    memset(d1a, 0, sizeof d1a);
    d1a[0] = 0x05;
    return pimax_send(h, 0x1a, 0x05, d1a, 10);
}

void pimax_set_video_rate(pimax_t *h, uint8_t fps) { atomic_store(&h->rate, fps); }
int pimax_is_connected(pimax_t *h) { return atomic_load(&h->connected); }

int pimax_wait_connected(pimax_t *h, int timeout_ms)
{
    struct timespec dl;
    clock_gettime(CLOCK_MONOTONIC, &dl);
    dl.tv_sec += timeout_ms / 1000;
    dl.tv_nsec += (long)(timeout_ms % 1000) * 1000000L;
    if (dl.tv_nsec >= 1000000000L) { dl.tv_sec++; dl.tv_nsec -= 1000000000L; }
    int rc = 0;
    pthread_mutex_lock(&h->lock);
    while (!atomic_load(&h->connected) && !rc)
        rc = pthread_cond_timedwait(&h->cond, &h->lock, &dl);
    int ok = atomic_load(&h->connected);
    pthread_mutex_unlock(&h->lock);
    return ok ? 0 : -ETIMEDOUT;
}

/* ---- session thread ---- */

static void set_state(pimax_t *h, pimax_state st)
{
    pthread_mutex_lock(&h->lock);
    atomic_store(&h->connected, st == PIMAX_CONNECTED);
    pthread_cond_broadcast(&h->cond);
    pthread_mutex_unlock(&h->lock);
    if (h->cb.on_state) h->cb.on_state(h->cb.user, st);
}

static void handle_log(pimax_t *h, int idx, const uint8_t *d, ssize_t n)
{
    char *b = h->logbuf[idx];
    size_t *len = &h->loglen[idx];
    for (ssize_t i = 0; i < n; i++) {
        uint8_t c = d[i];
        if (c == 0 || c == '\r') continue;
        if (c == '\n' || *len >= LOG_LINE_MAX - 1) {
            b[*len] = 0;
            if (*len && h->cb.on_log) h->cb.on_log(h->cb.user, b);
            *len = 0;
            if (c == '\n') continue;
        }
        b[(*len)++] = (char)c;
    }
}

static void handle_if0(pimax_t *h, const uint8_t *d)
{
    if (h->cb.on_reply) h->cb.on_reply(h->cb.user, d);
    if (d[0] != 0x4c) return;
    if (d[2] == 0x37 && (d[3] == PIMAX_SLOT_LEFT || d[3] == PIMAX_SLOT_RIGHT)) {
        pimax_controller_status s;
        memset(&s, 0, sizeof s);
        s.slot = d[3]; s.battery = d[9]; s.fw[0] = d[14]; s.fw[1] = d[15];
        memcpy(s.raw, d, PIMAX_REPORT_SIZE);
        if (h->cb.on_controller_status) h->cb.on_controller_status(h->cb.user, &s);
        return;
    }
    pthread_mutex_lock(&h->lock);
    if (h->pend_active && !h->pend_done && d[2] == h->pend_cmd && d[3] == h->pend_opt) {
        memcpy(h->pend_report, d, PIMAX_REPORT_SIZE);
        h->pend_done = 1;
        pthread_cond_broadcast(&h->cond);
    }
    pthread_mutex_unlock(&h->lock);
}

static void handle_report(pimax_t *h, int idx, const uint8_t *d, ssize_t n)
{
    if (n < 4) return;
    uint8_t r[PIMAX_REPORT_SIZE] = { 0 };
    memcpy(r, d, (size_t)n < sizeof r ? (size_t)n : sizeof r);
    if (idx == 3) { handle_log(h, idx, d, n); return; }
    if (idx == 0) { handle_if0(h, r); return; }
    pimax_imu_sample im[2];
    pimax_controller_report cr;
    if (pimax_decode_imu(r, im)) {
        if (h->cb.on_imu) { h->cb.on_imu(h->cb.user, &im[0]); h->cb.on_imu(h->cb.user, &im[1]); }
    } else if (pimax_decode_controller(r, &cr)) {
        if (h->cb.on_controller) h->cb.on_controller(h->cb.user, &cr);
    }
}

/* Returns when the device disappears or stop is set. */
static void run_session(pimax_t *h, char paths[PIMAX_NUM_IF][PIMAX_PATH_MAX], int mask)
{
    int fds[PIMAX_NUM_IF] = { -1, -1, -1, -1 };
    for (int i = 0; i < PIMAX_NUM_IF; i++) {
        if (!(mask & (1 << i))) continue;
        fds[i] = open(paths[i], (i == 0 ? O_RDWR : O_RDONLY) | O_NONBLOCK | O_CLOEXEC);
        if (fds[i] < 0 && (i == 0 || i == 2)) goto out;      /* must have IF0 and IF2 */
    }
    memset(h->loglen, 0, sizeof h->loglen);
    pthread_mutex_lock(&h->lock);
    h->ctl_fd = fds[0];
    pthread_mutex_unlock(&h->lock);

    static const uint8_t wake[] = { 0x01, 0x00, 0x00 };
    if (pimax_send(h, 0x1a, 0x05, wake, 3) < 0) goto out;      /* 58 06 1a 05 01 00 00 */
    set_state(h, PIMAX_CONNECTED);

    int64_t next_ka = now_ns();
    while (!atomic_load(&h->stop)) {
        int64_t now = now_ns();
        if (now >= next_ka) {
            next_ka += KEEPALIVE_NS;
            if (next_ka < now) next_ka = now + KEEPALIVE_NS;
            uint8_t rr[2] = { (uint8_t)atomic_load(&h->rate), 0 };
            int rc = pimax_send(h, 0x39, 0x00, rr, 2);
            if (rc == -ENODEV || rc == -EIO || rc == -ENOTCONN) break;
        }
        struct pollfd pf[PIMAX_NUM_IF];
        int map[PIMAX_NUM_IF], np = 0;
        for (int i = 0; i < PIMAX_NUM_IF; i++)
            if (fds[i] >= 0) { pf[np].fd = fds[i]; pf[np].events = POLLIN; pf[np].revents = 0; map[np++] = i; }
        int64_t wait = next_ka - now_ns();
        if (wait < 0) wait = 0;
        struct timespec ts = { wait / 1000000000LL, wait % 1000000000LL };
        int pr = ppoll(pf, np, &ts, NULL);
        if (pr < 0 && errno != EINTR) break;
        int gone = 0;
        for (int k = 0; k < np && pr > 0; k++) {
            if (pf[k].revents & POLLIN) {
                for (int rep = 0; rep < 32; rep++) {            /* drain */
                    uint8_t d[PIMAX_REPORT_SIZE];
                    ssize_t n = read(pf[k].fd, d, sizeof d);
                    if (n < 0) { if (errno != EAGAIN && errno != EINTR) gone = 1; break; }
                    if (n == 0) break;
                    handle_report(h, map[k], d, n);
                }
            }
            if (pf[k].revents & (POLLERR | POLLHUP | POLLNVAL)) gone = 1;
        }
        if (gone) break;
    }
out:
    pthread_mutex_lock(&h->lock);
    h->ctl_fd = -1;                 /* no writer can use the fd once we hold the lock */
    pthread_mutex_unlock(&h->lock);
    for (int i = 0; i < PIMAX_NUM_IF; i++)
        if (fds[i] >= 0) close(fds[i]);
    if (atomic_load(&h->connected)) set_state(h, PIMAX_DISCONNECTED);
}

static void *thread_main(void *arg)
{
    pimax_t *h = arg;
    while (!atomic_load(&h->stop)) {
        char paths[PIMAX_NUM_IF][PIMAX_PATH_MAX] = { { 0 } };
        int mask = pimax_find_interfaces(paths);
        if ((mask & 5) != 5) { sleep_ms(50); continue; }        /* need IF0 and IF2 */
        run_session(h, paths, mask);
        sleep_ms(100);
    }
    return NULL;
}

pimax_t *pimax_open(const pimax_callbacks *cb)
{
    pimax_t *h = calloc(1, sizeof *h);
    if (!h) return NULL;
    if (cb) h->cb = *cb;
    h->ctl_fd = -1;
    pthread_condattr_t ca;
    pthread_condattr_init(&ca);
    pthread_condattr_setclock(&ca, CLOCK_MONOTONIC);
    pthread_cond_init(&h->cond, &ca);
    pthread_condattr_destroy(&ca);
    pthread_mutex_init(&h->lock, NULL);
    pthread_mutex_init(&h->req_lock, NULL);
    if (pthread_create(&h->thread, NULL, thread_main, h)) { free(h); return NULL; }
    return h;
}

void pimax_close(pimax_t *h)
{
    if (!h) return;
    atomic_store(&h->stop, 1);
    pthread_join(h->thread, NULL);
    free(h);
}
