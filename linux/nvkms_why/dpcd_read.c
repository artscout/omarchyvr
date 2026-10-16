/*
 * dpcd_read: strictly read-only probe of a DP sink's capabilities.
 *   Stage 1 (default): NVKMS GET_DPY_ATTRIBUTE for every readable attribute of the headset dpy.
 *   Stage 2 (--rm):    RM client on /dev/nvidiactl: ROOT, ATTACH_IDS, DEVICE, SUBDEVICE, DISPLAY_COMMON,
 *                      SYSTEM_GET_SUPPORTED / GET_CONNECT_STATE / GET_EDID_V2 (all reads) and then
 *                      NV0073_CTRL_CMD_DP_AUXCH_CTRL with cmd = AUX | REQ_TYPE_READ ONLY.
 * Never issues AUX writes, I2C, modesets, ownership grabs.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>
#include "nvkms-api.h"
#include "nvkms-ioctl.h"
#undef opaqueDpyId
#undef opaqueDpyIdList
#include "nvos.h"
#include "class/cl0000.h"
#include "class/cl0080.h"
#include "class/cl2080.h"
#include "class/cl0073.h"
#include "ctrl/ctrl0000/ctrl0000gpu.h"
#include "ctrl/ctrl0073/ctrl0073system.h"
#include "ctrl/ctrl0073/ctrl0073specific.h"
#include "ctrl/ctrl0073/ctrl0073dp.h"
#include <linux/ioctl.h>

#define DRV_VERSION "610.57.04"
static int fd;

static int nvkms(enum NvKmsIoctlCommand cmd, void *p, size_t sz)
{
    struct NvKmsIoctlParams ip = { .cmd = cmd, .size = (NvU32)sz, .address = (NvU64)(NvUPtr)p };
    if (ioctl(fd, NVKMS_IOCTL_IOWR, &ip) < 0) return -1;
    return 0;
}

static const char *an[] = {
 "BACKLIGHT_BRIGHTNESS","SCANLINE","HW_HEAD","HEAD","REQUESTED_DITHERING","REQUESTED_DITHERING_MODE",
 "REQUESTED_DITHERING_DEPTH","CURRENT_DITHERING","CURRENT_DITHERING_MODE","CURRENT_DITHERING_DEPTH",
 "DIGITAL_VIBRANCE","REQUESTED_COLOR_FORMAT","CURRENT_COLOR_FORMAT","REQUESTED_COLOR_RANGE",
 "CURRENT_COLOR_RANGE","CURRENT_COLOR_BPC","DIGITAL_SIGNAL","DIGITAL_LINK_TYPE","DISPLAYPORT_LINK_RATE",
 "DISPLAYPORT_LINK_RATE_10MHZ","DISPLAYPORT_FORCE_ENABLE_FEC","FRAMELOCK_DISPLAY_CONFIG",
 "UPDATE_GLS_FRAMELOCK","RASTER_LOCK","UPDATE_FLIPLOCK","UPDATE_STEREO","DPMS","VRR_MIN_REFRESH_RATE",
 "DISPLAYPORT_CONNECTOR_TYPE","DISPLAYPORT_IS_MULTISTREAM","DISPLAYPORT_SINK_IS_AUDIO_CAPABLE",
 "NUMBER_OF_HARDWARE_HEADS_USED" };

static void stage1(void)
{
    struct NvKmsAllocDeviceParams *ad = calloc(1, sizeof(*ad));
    strncpy(ad->request.versionString, DRV_VERSION, sizeof(ad->request.versionString) - 1);
    ad->request.deviceId.rmDeviceId = 0;
    ad->request.enableConsoleHotplugHandling = 0;
    if (nvkms(NVKMS_IOCTL_ALLOC_DEVICE, ad, sizeof(*ad)) || ad->reply.status != NVKMS_ALLOC_DEVICE_STATUS_SUCCESS) {
        puts("NVKMS alloc device failed"); return; }
    NvKmsDeviceHandle dh = ad->reply.deviceHandle;
    for (NvU32 d = 0; d < ad->reply.numDisps; d++) {
        NvKmsDispHandle dp = ad->reply.dispHandles[d];
        struct NvKmsQueryDispParams qd = { .request = { dh, dp } };
        if (nvkms(NVKMS_IOCTL_QUERY_DISP, &qd, sizeof(qd))) continue;
        for (int b = 0; b < 32; b++) {
            NVDpyId id = { 1u << b };
            if (!(qd.reply.validDpys.opaqueDpyIdList & id.opaqueDpyId)) continue;
            struct NvKmsQueryDpyDynamicDataParams *dd = calloc(1, sizeof(*dd));
            dd->request.deviceHandle = dh; dd->request.dispHandle = dp; dd->request.dpyId = id;
            if (nvkms(NVKMS_IOCTL_QUERY_DPY_DYNAMIC_DATA, dd, sizeof(*dd)) || !dd->reply.connected) { free(dd); continue; }
            printf("NVKMS dpy 0x%08x \"%s\" maxPclk=%uKHz max=%ux%u vrHMD=%d dpGuidValid=%d guid=%s\n",
                   id.opaqueDpyId, dd->reply.name, dd->reply.maxPixelClockKHz, dd->reply.maxWidthInPixels,
                   dd->reply.maxHeightInPixels, dd->reply.isVirtualRealityHeadMountedDisplay,
                   dd->reply.dp.guid.valid, dd->reply.dp.guid.valid ? dd->reply.dp.guid.str : "-");
            free(dd);
            for (unsigned a = 0; a <= NV_KMS_DPY_ATTRIBUTE_NUMBER_OF_HARDWARE_HEADS_USED; a++) {
                struct NvKmsGetDpyAttributeParams g;
                memset(&g, 0, sizeof(g));
                g.request.deviceHandle = dh; g.request.dispHandle = dp; g.request.dpyId = id; g.request.attribute = a;
                if (nvkms(NVKMS_IOCTL_GET_DPY_ATTRIBUTE, &g, sizeof(g)) || 0)
                    printf("   attr %-36s (unreadable: %s)\n", an[a], strerror(errno));
                else printf("   attr %-36s = %lld\n", an[a], (long long)g.reply.value);
            }
        }
    }
    struct NvKmsFreeDeviceParams fp = { .request = { dh } };
    nvkms(NVKMS_IOCTL_FREE_DEVICE, &fp, sizeof(fp));
}

/* ---------------- RM ---------------- */
static int rfd;
#define NVESC(n, T) _IOWR('F', n, T)
static NvHandle hClient, hDev = 0x1d0001, hSub = 0x1d0002, hDisp = 0x1d0003;

static int rm_alloc(NvHandle parent, NvHandle obj, NvU32 cls, void *p, NvU32 sz, int tag)
{
    NVOS21_PARAMETERS a; memset(&a, 0, sizeof(a));
    a.hRoot = hClient; a.hObjectParent = parent; a.hObjectNew = obj; a.hClass = cls;
    a.pAllocParms = (NvP64)(NvUPtr)p; a.paramsSize = sz;
    if (ioctl(rfd, NVESC(0x2B, NVOS21_PARAMETERS), &a) < 0) { printf("RM alloc(%s) ioctl: %s\n", "x", strerror(errno)); return -1; }
    if (cls == NV01_ROOT) hClient = a.hObjectNew;
    if (a.status) printf("RM alloc class 0x%x status 0x%x\n", cls, a.status);
    (void)tag;
    return a.status;
}
static int rm_ctl(NvHandle obj, NvU32 cmd, void *p, NvU32 sz, NvU32 *st)
{
    NVOS54_PARAMETERS c; memset(&c, 0, sizeof(c));
    c.hClient = hClient; c.hObject = obj; c.cmd = cmd; c.params = (NvP64)(NvUPtr)p; c.paramsSize = sz;
    if (ioctl(rfd, NVESC(0x2A, NVOS54_PARAMETERS), &c) < 0) { printf("RM ctl 0x%x ioctl: %s\n", cmd, strerror(errno)); return -1; }
    if (st) *st = c.status;
    return c.status;
}

static void dump(NvU32 base, const NvU8 *b, int n, const int *ok)
{
    for (int i = 0; i < n; i += 16) {
        printf("  %05x:", base + i);
        for (int j = i; j < i + 16 && j < n; j++) ok[j] ? printf(" %02x", b[j]) : printf(" ??");
        puts("");
    }
}

static int aux_read(NvU32 disp, NvU32 addr, NvU8 *out, int n, int *st)
{
    /* read in chunks of <=16, retry per chunk */
    for (int off = 0; off < n; ) {
        int chunk = n - off > 16 ? 16 : n - off;
        NV0073_CTRL_DP_AUXCH_CTRL_PARAMS p; memset(&p, 0, sizeof(p));
        p.displayId = disp;
        p.cmd = (NV0073_CTRL_DP_AUXCH_CMD_TYPE_AUX << 3) | NV0073_CTRL_DP_AUXCH_CMD_REQ_TYPE_READ; /* READ only */
        /* the control takes size 0-based and returns the completed byte count (dp_evoadapter.cpp) */
        p.addr = addr + off; p.size = chunk - 1; p.retryTimeMs = 0;
        NvU32 s = 0;
        if (rm_ctl(hDisp, NV0073_CTRL_CMD_DP_AUXCH_CTRL, &p, sizeof(p), &s) < 0) return -1;
        if (s || p.replyType != 0) { *st = s ? (int)s : (int)(0x10000 | p.replyType); return off; }
        int got = p.size > (NvU32)chunk ? chunk : (int)p.size;
        if (got == 0) { *st = 0x20000; return off; }
        memcpy(out + off, p.data, got);
        off += got;
    }
    return n;
}

static void stage2(void)
{
    rfd = open("/dev/nvidiactl", O_RDWR);
    if (rfd < 0) { perror("open nvidiactl"); return; }
    if (open("/dev/nvidia0", O_RDWR) < 0) perror("open nvidia0"); /* RM requires the GPU node open in-process */
    NV0000_ALLOC_PARAMETERS rp; memset(&rp, 0, sizeof(rp));
    if (rm_alloc(0, 0, NV01_ROOT, &rp, sizeof(rp), 0)) { puts("root alloc failed"); return; }
    printf("RM client 0x%x\n", hClient);
    NvU32 st;
    NV0000_CTRL_GPU_GET_PROBED_IDS_PARAMS *pi = calloc(1, sizeof(*pi));
    if (rm_ctl(hClient, NV0000_CTRL_CMD_GPU_GET_PROBED_IDS, pi, sizeof(*pi), &st) || st) { printf("probed ids st=0x%x\n", st); return; }
    NvU32 gid = pi->gpuIds[0];
    printf("gpuId[0]=0x%x\n", gid);
    NV0000_CTRL_GPU_ATTACH_IDS_PARAMS *ai = calloc(1, sizeof(*ai));
    for (int i = 0; i < NV0000_CTRL_GPU_MAX_PROBED_GPUS; i++) ai->gpuIds[i] = NV0000_CTRL_GPU_INVALID_ID;
    ai->gpuIds[0] = gid;
    rm_ctl(hClient, NV0000_CTRL_CMD_GPU_ATTACH_IDS, ai, sizeof(*ai), &st);
    printf("attach ids st=0x%x\n", st);
    NV0080_ALLOC_PARAMETERS dp; memset(&dp, 0, sizeof(dp));
    dp.deviceId = 0; dp.hClientShare = hClient;
    if (rm_alloc(hClient, hDev, NV01_DEVICE_0, &dp, sizeof(dp), 1)) return;
    NV2080_ALLOC_PARAMETERS sp = { 0 };
    if (rm_alloc(hDev, hSub, NV20_SUBDEVICE_0, &sp, sizeof(sp), 2)) return;
    if (rm_alloc(hDev, hDisp, NV04_DISPLAY_COMMON, NULL, 0, 3)) return;
    puts("display common allocated");
    NV0073_CTRL_SYSTEM_GET_SUPPORTED_PARAMS sup = { 0 };
    if (rm_ctl(hDisp, NV0073_CTRL_CMD_SYSTEM_GET_SUPPORTED, &sup, sizeof(sup), &st) || st) { printf("get_supported st=0x%x\n", st); return; }
    NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_PARAMS cs = { 0 };
    cs.displayMask = sup.displayMask;
    cs.flags = NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_FLAGS_METHOD_CACHED; /* no new detection */
    rm_ctl(hDisp, NV0073_CTRL_CMD_SYSTEM_GET_CONNECT_STATE, &cs, sizeof(cs), &st);
    printf("supported=0x%x connected=0x%x (st=0x%x)\n", sup.displayMask, cs.displayMask, st);
    {
        NV0073_CTRL_CMD_DP_GET_CAPS_PARAMS *gc = calloc(1, sizeof(*gc));
        for (int sor = 0; sor < 4; sor++) {
            memset(gc, 0, sizeof(*gc)); gc->sorIndex = sor;
            if (rm_ctl(hDisp, NV0073_CTRL_CMD_DP_GET_CAPS, gc, sizeof(*gc), &st) || st) { printf("GPU DP_GET_CAPS sor%d st=0x%x\n", sor, st); continue; }
            printf("GPU-side DP_GET_CAPS sor%d: dpVer=0x%x maxLinkRate=%u UHBR=0x%x MST=%d FEC=%d | DSC enc: supported=%d colorFmtMask=0x%x lineBufKB=%u rateBufKB=%u bppPrec=%u maxHztSlices=%u lineBufBitDepth=%u\n",
                   sor, gc->dpVersionsSupported, gc->maxLinkRate, gc->UHBRSupportedByGpu, gc->bIsMultistreamSupported, gc->bFECSupported,
                   gc->DSC.bDscSupported, gc->DSC.encoderColorFormatMask, gc->DSC.lineBufferSizeKB, gc->DSC.rateBufferSizeKB,
                   gc->DSC.bitsPerPixelPrecision, gc->DSC.maxNumHztSlices, gc->DSC.lineBufferBitDepth);
        }
    }
    static const struct { NvU32 a, n; const char *nm; } rg[] = {
        {0x0000,16,"rev/rates/lanes"},{0x0020,16,"0x20"},{0x0060,16,"DSC"},{0x0090,16,"FEC"},
        {0x2200,16,"ext caps"},{0x0400,16,"sink OUI/ID 0x400"},{0x0500,16,"branch OUI/ID 0x500"} };
    for (int bit = 0; bit < 32; bit++) {
        NvU32 id = 1u << bit;
        if (!(cs.displayMask & id)) continue;
        NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS *e = calloc(1, sizeof(*e));
        e->displayId = id; e->flags = NV0073_CTRL_SPECIFIC_GET_EDID_FLAGS_COPY_CACHE_YES;
        char mfg[4] = "???"; int is_pimax = 0;
        if (!rm_ctl(hDisp, NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2, e, sizeof(*e), &st) && !st && e->bufferSize >= 128) {
            unsigned v = (e->edidBuffer[8] << 8) | e->edidBuffer[9];
            mfg[0] = 'A' - 1 + ((v >> 10) & 31); mfg[1] = 'A' - 1 + ((v >> 5) & 31); mfg[2] = 'A' - 1 + (v & 31); mfg[3] = 0;
            for (int o = 54; o < 126; o += 18) if (e->edidBuffer[o+3] == 0xfc)
                printf("  name: %.13s\n", &e->edidBuffer[o+5]);
        }
        printf("=== displayId 0x%x EDID mfg=%s edidBytes=%u\n", id, mfg, e->bufferSize);
        free(e);
        (void)is_pimax;
        for (unsigned r = 0; r < sizeof(rg)/sizeof(rg[0]); r++) {
            NvU8 buf[16] = {0}; int ok[16] = {0}; int s = 0;
            int got = aux_read(id, rg[r].a, buf, rg[r].n, &s);
            for (int i = 0; i < got && i < 16; i++) ok[i] = 1;
            printf(" [%s] read %d/%u bytes%s\n", rg[r].nm, got, rg[r].n, got < (int)rg[r].n ? " (stopped, status/reply code shown next)" : "");
            if (got < (int)rg[r].n) printf("   status=0x%x\n", s);
            dump(rg[r].a, buf, 16, ok);
        }
    }
}

int main(int argc, char **argv)
{
    int rm = argc > 1 && !strcmp(argv[1], "--rm");
    int nk = !(argc > 1 && !strcmp(argv[1], "--rm-only"));
    if (argc > 1 && !strcmp(argv[1], "--rm-only")) rm = 1;
    if (nk) {
        fd = open("/dev/nvidia-modeset", O_RDWR);
        if (fd < 0) perror("open nvidia-modeset"); else { stage1(); close(fd); }
    }
    if (rm) stage2();
    return 0;
}
