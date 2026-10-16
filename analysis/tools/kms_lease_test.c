/* KMS lease test: drive Pimax DP-1 via wp_drm_lease_v1 + plain atomic KMS (dumb buffer), no Vulkan.
 * Left half (x<W/2) RED, right half GREEN. Usage: kms_lease_test [refresh=72] [seconds=25]
 * Build: see bottom comment. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <errno.h>
#include <sys/mman.h>
#include <wayland-client.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <drm_fourcc.h>
#include "drm-lease-v1-client-protocol.h"

static struct wp_drm_lease_device_v1 *dev;
static struct wp_drm_lease_connector_v1 *dp1;
static int lease_fd = -1, finished, dev_done;
static struct { struct wp_drm_lease_connector_v1 *c; char name[64]; uint32_t id; } conns[16];
static int nconn;

static void d_fd(void *d, struct wp_drm_lease_device_v1 *x, int32_t fd) { close(fd); }
static void d_conn(void *d, struct wp_drm_lease_device_v1 *x, struct wp_drm_lease_connector_v1 *c);
static void d_done(void *d, struct wp_drm_lease_device_v1 *x) { dev_done = 1; }
static void d_rel(void *d, struct wp_drm_lease_device_v1 *x) {}
static const struct wp_drm_lease_device_v1_listener dl = { d_fd, d_conn, d_done, d_rel };
static void c_name(void *d, struct wp_drm_lease_connector_v1 *c, const char *n) {
    for (int i = 0; i < nconn; i++) if (conns[i].c == c) snprintf(conns[i].name, 64, "%s", n); }
static void c_desc(void *d, struct wp_drm_lease_connector_v1 *c, const char *n) {}
static void c_id(void *d, struct wp_drm_lease_connector_v1 *c, uint32_t id) {
    for (int i = 0; i < nconn; i++) if (conns[i].c == c) conns[i].id = id; }
static void c_done(void *d, struct wp_drm_lease_connector_v1 *c) {}
static void c_wd(void *d, struct wp_drm_lease_connector_v1 *c) {}
static const struct wp_drm_lease_connector_v1_listener cl = { c_name, c_desc, c_id, c_done, c_wd };
static void d_conn(void *d, struct wp_drm_lease_device_v1 *x, struct wp_drm_lease_connector_v1 *c) {
    if (nconn < 16) { conns[nconn].c = c; wp_drm_lease_connector_v1_add_listener(c, &cl, NULL); nconn++; } }
static void l_fd(void *d, struct wp_drm_lease_v1 *l, int32_t fd) { lease_fd = fd; }
static void l_fin(void *d, struct wp_drm_lease_v1 *l) { finished = 1; }
static const struct wp_drm_lease_v1_listener ll = { l_fd, l_fin };
static void reg_g(void *d, struct wl_registry *r, uint32_t name, const char *iface, uint32_t ver) {
    if (!strcmp(iface, wp_drm_lease_device_v1_interface.name) && !dev) {
        dev = wl_registry_bind(r, name, &wp_drm_lease_device_v1_interface, 1);
        wp_drm_lease_device_v1_add_listener(dev, &dl, NULL); } }
static void reg_r(void *d, struct wl_registry *r, uint32_t n) {}
static const struct wl_registry_listener rl = { reg_g, reg_r };

static uint32_t prop(int fd, uint32_t obj, uint32_t type, const char *name) {
    drmModeObjectProperties *p = drmModeObjectGetProperties(fd, obj, type);
    uint32_t id = 0;
    for (uint32_t i = 0; p && i < p->count_props; i++) {
        drmModePropertyRes *pr = drmModeGetProperty(fd, p->props[i]);
        if (pr && !strcmp(pr->name, name)) id = pr->prop_id;
        drmModeFreeProperty(pr);
    }
    drmModeFreeObjectProperties(p);
    return id;
}
static uint64_t propval(int fd, uint32_t obj, uint32_t type, const char *name) {
    drmModeObjectProperties *p = drmModeObjectGetProperties(fd, obj, type);
    uint64_t v = 0;
    for (uint32_t i = 0; p && i < p->count_props; i++) {
        drmModePropertyRes *pr = drmModeGetProperty(fd, p->props[i]);
        if (pr && !strcmp(pr->name, name)) v = p->prop_values[i];
        drmModeFreeProperty(pr);
    }
    drmModeFreeObjectProperties(p);
    return v;
}

static uint32_t *fb; static int W, H, pitch4;
static void rect(int x0, int y0, int x1, int y1, uint32_t c) {
    for (int y = y0; y < y1; y++) for (int x = x0; x < x1; x++) if (x >= 0 && x < W && y >= 0 && y < H) fb[(size_t)y * pitch4 + x] = c; }
static void arrow(int cx, int cy, uint32_t c) { /* points up, head 1200 wide x 900 tall */
    for (int y = 0; y < 900; y++) { int hw = y * 600 / 900; rect(cx - hw, cy - 1000 + y, cx + hw, cy - 1000 + y + 1, c); }
    rect(cx - 150, cy - 100, cx + 150, cy + 900, c); }
static void draw_half(int x0, int hw, int isR) {
    uint32_t bg = isR ? 0x0000b000 : 0x00d00000, wh = 0x00ffffff, ye = 0x00ffff00;
    rect(x0, 0, x0 + hw, H, bg);
    rect(x0 + 40, 40, x0 + 440, 440, ye);
    arrow(x0 + hw / 2, 1400, wh);
    int bx = x0 + 150, by = H - 1250; /* glyph box 900x1100 in lower-left */
    if (!isR) { rect(bx, by, bx + 300, by + 1100, wh); rect(bx, by + 800, bx + 900, by + 1100, wh); }
    else { rect(bx, by, bx + 300, by + 1100, wh); rect(bx, by, bx + 800, by + 250, wh);
           rect(bx, by + 450, bx + 800, by + 700, wh); rect(bx + 550, by, bx + 800, by + 700, wh);
           for (int y = 700; y < 1100; y++) { int x = bx + 300 + (y - 700) * 550 / 400; rect(x, by + y, x + 280, by + y + 1, wh); } }
}

int main(int argc, char **argv) {
    int hz = argc > 1 ? atoi(argv[1]) : 72, secs = argc > 2 ? atoi(argv[2]) : 25;
    struct wl_display *dpy = wl_display_connect(NULL);
    if (!dpy) { fprintf(stderr, "no wayland display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(dpy);
    wl_registry_add_listener(reg, &rl, NULL);
    wl_display_roundtrip(dpy);
    if (!dev) { fprintf(stderr, "no wp_drm_lease_device_v1\n"); return 1; }
    wl_display_roundtrip(dpy); wl_display_roundtrip(dpy);
    for (int i = 0; i < nconn; i++) { printf("lease connector: %s id=%u\n", conns[i].name, conns[i].id); if (!strcmp(conns[i].name, "DP-1")) dp1 = conns[i].c; }
    if (!dp1) { fprintf(stderr, "DP-1 not offered\n"); return 1; }
    struct wp_drm_lease_request_v1 *rq = wp_drm_lease_device_v1_create_lease_request(dev);
    wp_drm_lease_request_v1_request_connector(rq, dp1);
    struct wp_drm_lease_v1 *lease = wp_drm_lease_request_v1_submit(rq);
    wp_drm_lease_v1_add_listener(lease, &ll, NULL);
    while (lease_fd < 0 && !finished) if (wl_display_roundtrip(dpy) < 0) break;
    if (lease_fd < 0) { fprintf(stderr, "LEASE FAILED (finished=%d)\n", finished); return 1; }
    printf("LEASE OK fd=%d\n", lease_fd);
    int fd = lease_fd;
    drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1);
    if (drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1)) { perror("atomic cap"); return 1; }
    drmModeRes *res = drmModeGetResources(fd);
    drmModeConnector *con = NULL;
    for (int i = 0; res && i < res->count_connectors; i++) {
        drmModeConnector *c = drmModeGetConnector(fd, res->connectors[i]);
        if (c && c->connection == DRM_MODE_CONNECTED) { con = c; break; } drmModeFreeConnector(c); }
    if (!con) { fprintf(stderr, "no connected leased connector\n"); return 1; }
    printf("connector %u, %d modes, crtcs=%d\n", con->connector_id, con->count_modes, res->count_crtcs);
    drmModeModeInfo *m = NULL;
    for (int i = 0; i < con->count_modes; i++) {
        drmModeModeInfo *x = &con->modes[i];
        printf("  mode %d: %dx%d@%d\n", i, x->hdisplay, x->vdisplay, x->vrefresh);
        if (x->hdisplay == 7104 && x->vdisplay == 3840 && x->vrefresh == hz && !m) m = x; }
    if (!m) { fprintf(stderr, "mode 7104x3840@%d not found\n", hz); return 1; }
    W = m->hdisplay; H = m->vdisplay;
    /* crtc: via encoder or first possible */
    uint32_t crtc_id = 0; int crtc_idx = 0;
    drmModeEncoder *enc = con->encoder_id ? drmModeGetEncoder(fd, con->encoder_id) : NULL;
    for (int i = 0; i < res->count_crtcs; i++) {
        if (enc && enc->crtc_id == res->crtcs[i]) { crtc_id = res->crtcs[i]; crtc_idx = i; break; } }
    if (!crtc_id) for (int e = 0; e < con->count_encoders && !crtc_id; e++) {
        drmModeEncoder *en = drmModeGetEncoder(fd, con->encoders[e]);
        for (int i = 0; en && i < res->count_crtcs; i++) if (en->possible_crtcs & (1u << i)) { crtc_id = res->crtcs[i]; crtc_idx = i; break; }
        drmModeFreeEncoder(en); }
    uint32_t plane_id = 0;
    drmModePlaneRes *pr = drmModeGetPlaneResources(fd);
    for (uint32_t i = 0; pr && i < pr->count_planes; i++) {
        drmModePlane *p = drmModeGetPlane(fd, pr->planes[i]);
        if (p && (p->possible_crtcs & (1u << crtc_idx)) && propval(fd, p->plane_id, DRM_MODE_OBJECT_PLANE, "type") == DRM_PLANE_TYPE_PRIMARY) plane_id = p->plane_id;
        drmModeFreePlane(p); }
    printf("crtc=%u plane=%u mode=%dx%d@%d\n", crtc_id, plane_id, W, H, m->vrefresh);
    if (!crtc_id || !plane_id) { fprintf(stderr, "no crtc/plane in lease\n"); return 1; }

    struct drm_mode_create_dumb cd = { .width = W, .height = H, .bpp = 32 };
    if (drmIoctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &cd)) { perror("create dumb"); return 1; }
    pitch4 = cd.pitch / 4;
    uint32_t fbid;
    uint32_t handles[4] = { cd.handle }, pitches[4] = { cd.pitch }, offs[4] = {0};
    if (drmModeAddFB2(fd, W, H, DRM_FORMAT_XRGB8888, handles, pitches, offs, &fbid, 0)) { perror("addfb2"); return 1; }
    struct drm_mode_map_dumb md = { .handle = cd.handle };
    drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &md);
    fb = mmap(0, cd.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, md.offset);
    if (fb == MAP_FAILED) { perror("mmap"); return 1; }
    draw_half(0, W / 2, 0); draw_half(W / 2, W / 2, 1);
    printf("buffer filled (L red / R green), fb=%u\n", fbid);

    uint32_t blob;
    drmModeCreatePropertyBlob(fd, m, sizeof *m, &blob);
    drmModeAtomicReq *rqa = drmModeAtomicAlloc();
#define ADD(obj, type, nm, v) drmModeAtomicAddProperty(rqa, obj, prop(fd, obj, type, nm), v)
    ADD(con->connector_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", crtc_id);
    ADD(crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", blob);
    ADD(crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", fbid);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID", crtc_id);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "SRC_X", 0); ADD(plane_id, DRM_MODE_OBJECT_PLANE, "SRC_Y", 0);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "SRC_W", (uint64_t)W << 16); ADD(plane_id, DRM_MODE_OBJECT_PLANE, "SRC_H", (uint64_t)H << 16);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_X", 0); ADD(plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_Y", 0);
    ADD(plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W", W); ADD(plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_H", H);

    char cmd[256];
    snprintf(cmd, sizeof cmd, "python3 /home/user/pimax-re/linux/pimax_video_on.py %d 40 > /tmp/kms-test-fwlog.txt 2>&1 &", hz);
    system(cmd); sleep(2);
    int r = drmModeAtomicCommit(fd, rqa, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL);
    if (r) { fprintf(stderr, "ATOMIC COMMIT FAILED: %s\n", strerror(errno)); }
    else printf("ATOMIC COMMIT OK (modeset)\n");
    fflush(stdout);
    if (!r) {
        system("echo '--- /sys/class/drm/card1-DP-1/modes'; cat /sys/class/drm/card1-DP-1/modes");
        printf("Suggest (needs root, not run): sudo cat /sys/kernel/debug/dri/1/state   # plane src/dst\n");
        fflush(stdout);
        for (int i = 0; i < secs; i++) { wl_display_dispatch_pending(dpy); wl_display_flush(dpy); sleep(1); }
    }
    drmModeAtomicFree(rqa);
    drmModeRmFB(fd, fbid); munmap(fb, cd.size);
    wp_drm_lease_v1_destroy(lease);   /* releases the lease */
    wl_display_roundtrip(dpy);
    close(fd);
    wl_display_disconnect(dpy);
    printf("lease released, exit\n");
    return r ? 1 : 0;
}
/* Build (from this dir):
 *  wayland-scanner client-header /usr/share/wayland-protocols/staging/drm-lease/drm-lease-v1.xml drm-lease-v1-client-protocol.h
 *  wayland-scanner private-code  /usr/share/wayland-protocols/staging/drm-lease/drm-lease-v1.xml drm-lease-v1-protocol.c
 *  gcc -O2 -I. -o kms_lease_test kms_lease_test.c drm-lease-v1-protocol.c $(pkg-config --cflags --libs libdrm wayland-client)
 */
