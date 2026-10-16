/* Stands in for pimax_hmd.c / pimax_ctrl.c: uses the same libpimax entry points, only without headset (empty sysfs). */
#include <stdio.h>
#include "pimax.h"

static int n_state;
static void on_state(void *u, pimax_state s) { (void)u; (void)s; n_state++; }

int drv_probe(void)
{
    char paths[PIMAX_NUM_IF][PIMAX_PATH_MAX];
    int mask = pimax_find_interfaces(paths);
    pimax_callbacks cb = { .on_state = on_state };
    pimax_t *h = pimax_open(&cb);
    if (!h) return -1;
    int rc = pimax_wait_connected(h, 200);      /* no headset: -ETIMEDOUT (-110) */
    int send = pimax_send(h, 0x03, 0, NULL, 0); /* not connected: -ENOTCONN (-107) */
    pimax_close(h);
    printf("mask=%d wait=%d send=%d\n", mask, rc, send);
    return (rc == -110 && send == -107) ? 0 : 1;
}
