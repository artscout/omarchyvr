/*
 * nvkms_why: read-only NVKMS mode-validation diagnostic.
 *
 * Calls ONLY: ALLOC_DEVICE, QUERY_DISP, QUERY_DPY_STATIC_DATA (not used),
 * QUERY_DPY_DYNAMIC_DATA (all force/override/ignore flags = FALSE, so no state change),
 * VALIDATE_MODE_INDEX, FREE_DEVICE.
 * NEVER called: GRAB_OWNERSHIP, SET_MODE, any SET_*_ATTRIBUTE, overrideEdid.
 * VALIDATE_MODE_INDEX has no EDID parameter, so a caller-supplied EDID cannot be
 * validated without overrideEdid (which mutates shared dpy state): not supported here.
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

#define DRV_VERSION "610.57.04"

static int fd;

static int nvkms(enum NvKmsIoctlCommand cmd, void *p, size_t sz)
{
    struct NvKmsIoctlParams ip = { .cmd = cmd, .size = (NvU32)sz,
                                   .address = (NvU64)(NvUPtr)p };
    if (ioctl(fd, NVKMS_IOCTL_IOWR, &ip) < 0) {
        fprintf(stderr, "ioctl cmd %d: %s\n", cmd, strerror(errno));
        return -1;
    }
    return 0;
}

int main(int argc, char **argv)
{
    NvU32 ov = 0;
    enum NvKmsDscMode dsc = NVKMS_DSC_MODE_DEFAULT;
    NvBool verbose = 1, more = 1, quiet_invalid = 0;
    int dev = 0;
    const char *want = NULL;      /* substring filter on dpy name; default "DP" */
    int only_w = 0, only_h = 0, sweep = 0;
    int hb_fp = 0, hb_sync = 0, hb_bp = 0;   /* --timing HZ,HA,HFP,HS,HBP,VA,VFP,VS,VBP: validate one custom mode */
    int va = 0, vfp = 0, vsw = 0, vbp = 0, ha = 0; double hz = 0;

    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "--no-dp-bw")) ov |= NVKMS_MODE_VALIDATION_NO_DISPLAYPORT_BANDWIDTH_CHECK;
        else if (!strcmp(a, "--no-ext-gpu-caps")) ov |= NVKMS_MODE_VALIDATION_NO_EXTENDED_GPU_CAPABILITIES_CHECK;
        else if (!strcmp(a, "--no-max-pclk")) ov |= NVKMS_MODE_VALIDATION_NO_MAX_PCLK_CHECK;
        else if (!strcmp(a, "--no-edid-max-pclk")) ov |= NVKMS_MODE_VALIDATION_NO_EDID_MAX_PCLK_CHECK;
        else if (!strcmp(a, "--no-edid-dfp-size")) ov |= NVKMS_MODE_VALIDATION_NO_EDID_DFP_MAX_SIZE_CHECK;
        else if (!strcmp(a, "--no-max-size")) ov |= NVKMS_MODE_VALIDATION_NO_MAX_SIZE_CHECK;
        else if (!strcmp(a, "--no-total-size")) ov |= NVKMS_MODE_VALIDATION_NO_TOTAL_SIZE_CHECK;
        else if (!strcmp(a, "--no-horiz-sync")) ov |= NVKMS_MODE_VALIDATION_NO_HORIZ_SYNC_CHECK;
        else if (!strcmp(a, "--no-vert-refresh")) ov |= NVKMS_MODE_VALIDATION_NO_VERT_REFRESH_CHECK;
        else if (!strcmp(a, "--allow-non-edid")) ov |= NVKMS_MODE_VALIDATION_ALLOW_NON_EDID_MODES;
        else if (!strcmp(a, "--max-one-head")) ov |= NVKMS_MODE_VALIDATION_MAX_ONE_HARDWARE_HEAD;
        else if (!strcmp(a, "--dsc-force")) dsc = NVKMS_DSC_MODE_FORCE_ENABLE;
        else if (!strcmp(a, "--dsc-off")) dsc = NVKMS_DSC_MODE_FORCE_DISABLE;
        else if (!strcmp(a, "--terse")) verbose = more = 0;
        else if (!strcmp(a, "--dpy") && i + 1 < argc) want = argv[++i];
        else if (!strcmp(a, "--dev") && i + 1 < argc) dev = atoi(argv[++i]);
        else if (!strcmp(a, "--size") && i + 1 < argc) sscanf(argv[++i], "%dx%d", &only_w, &only_h);
        else if (!strcmp(a, "--invalid-only")) quiet_invalid = 1;
        else if (!strcmp(a, "--sweep")) sweep = 1;
        else if (!strcmp(a, "--timing") && i + 1 < argc) {
            if (sscanf(argv[++i], "%lf,%d,%d,%d,%d,%d,%d,%d,%d", &hz, &ha, &hb_fp, &hb_sync, &hb_bp,
                       &va, &vfp, &vsw, &vbp) != 9) return 2;
        }
        else {
            fprintf(stderr, "usage: %s [--no-dp-bw] [--no-ext-gpu-caps] [--no-max-pclk] [--no-edid-max-pclk]\n"
                "  [--no-edid-dfp-size] [--no-max-size] [--no-total-size] [--no-horiz-sync] [--no-vert-refresh]\n"
                "  [--allow-non-edid] [--max-one-head] [--dsc-force|--dsc-off] [--terse]\n"
                "  [--dpy substr] [--dev N] [--size WxH] [--invalid-only] [--sweep]\n", argv[0]);
            return 2;
        }
    }
    if (!want) want = "DP";

    fd = open("/dev/nvidia-modeset", O_RDWR);
    if (fd < 0) { perror("open /dev/nvidia-modeset"); return 1; }

    struct NvKmsAllocDeviceParams *ad = calloc(1, sizeof(*ad));
    strncpy(ad->request.versionString, DRV_VERSION, sizeof(ad->request.versionString) - 1);
    ad->request.deviceId.rmDeviceId = dev;
    ad->request.enableConsoleHotplugHandling = 0; /* leave hotplug behaviour unchanged */
    if (nvkms(NVKMS_IOCTL_ALLOC_DEVICE, ad, sizeof(*ad))) return 1;
    if (ad->reply.status != NVKMS_ALLOC_DEVICE_STATUS_SUCCESS) {
        fprintf(stderr, "ALLOC_DEVICE status %d (1=version mismatch)\n", ad->reply.status);
        return 1;
    }
    NvKmsDeviceHandle dh = ad->reply.deviceHandle;
    printf("device handle %u, disps %u, heads %u, driver API ver %s\n", dh,
           ad->reply.numDisps, ad->reply.numHeads, DRV_VERSION);
    printf("overrides=0x%x dscMode=%d\n", ov, dsc);

    int rc = 0;
    for (NvU32 d = 0; d < ad->reply.numDisps; d++) {
        NvKmsDispHandle dp = ad->reply.dispHandles[d];
        struct NvKmsQueryDispParams qd = { .request = { dh, dp } };
        if (nvkms(NVKMS_IOCTL_QUERY_DISP, &qd, sizeof(qd))) { rc = 1; continue; }
        printf("disp %u: %s validDpys=0x%08x\n", d, qd.reply.gpuString,
               qd.reply.validDpys.opaqueDpyIdList);

        for (int b = 0; b < 32; b++) {
            NVDpyId id = { 1u << b };
            if (!(qd.reply.validDpys.opaqueDpyIdList & id.opaqueDpyId)) continue;
            struct NvKmsQueryDpyDynamicDataParams *dd = calloc(1, sizeof(*dd));
            dd->request.deviceHandle = dh; dd->request.dispHandle = dp;
            dd->request.dpyId = id;   /* all force/override flags stay FALSE */
            if (nvkms(NVKMS_IOCTL_QUERY_DPY_DYNAMIC_DATA, dd, sizeof(*dd))) { free(dd); rc = 1; continue; }
            printf("  dpy 0x%08x name=%s connected=%d maxPclkKHz=%u maxW=%u maxH=%u vrHMD=%d edid=%u bytes valid=%d\n",
                   id.opaqueDpyId, dd->reply.name, dd->reply.connected,
                   dd->reply.maxPixelClockKHz, dd->reply.maxWidthInPixels,
                   dd->reply.maxHeightInPixels, dd->reply.isVirtualRealityHeadMountedDisplay,
                   dd->reply.edid.bufferSize, dd->reply.edid.valid);
            int hit = dd->reply.connected && strstr(dd->reply.name, want);
            if (hit) {
                printf("  >>> EDID infoString of %s:\n%s\n", dd->reply.name, dd->reply.edid.infoString);
            }
            free(dd);
            if (!hit) continue;

            printf("  >>> validating modes of %s\n", "selected dpy");
            char *info = malloc(NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH);
            for (NvU32 mi = 0; mi < 4096; mi++) {
                struct NvKmsValidateModeIndexParams vp;
                memset(&vp, 0, sizeof(vp));
                memset(info, 0, NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH);
                vp.request.deviceHandle = dh;
                vp.request.dispHandle = dp;
                vp.request.dpyId = id;
                vp.request.modeIndex = mi;
                vp.request.modeValidation.verboseModeValidation = verbose;
                vp.request.modeValidation.moreVerboseModeValidation = more;
                vp.request.modeValidation.overrides = ov;
                vp.request.modeValidation.dscMode = dsc;
                vp.request.infoStringSize = NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH;
                vp.request.pInfoString = (NvU64)(NvUPtr)info;
                if (nvkms(NVKMS_IOCTL_VALIDATE_MODE_INDEX, &vp, sizeof(vp))) { rc = 1; break; }
                if (vp.reply.end) { printf("  (end at index %u)\n", mi); break; }
                NvModeTimings *t = &vp.reply.mode.timings;
                if ((only_w && t->hVisible != only_w) || (only_h && t->vVisible != only_h)) continue;
                if (sweep) {
                    /* Trim vertical lines off this EDID mode (pclk scaled to keep refresh) and ask
                     * NVKMS_IOCTL_VALIDATE_MODE (no state change; needs ALLOW_NON_EDID_MODES). */
                    static const int hs[] = { 3840, 3600, 3200, 2880, 2400, 2160, 1840, 1200 };
                    for (unsigned k = 0; k < sizeof(hs)/sizeof(hs[0]); k++) {
                        struct NvKmsValidateModeParams *vm = calloc(1, sizeof(*vm));
                        int trim = (int)t->vVisible - hs[k];
                        vm->request.deviceHandle = dh; vm->request.dispHandle = dp; vm->request.dpyId = id;
                        vm->request.modeValidation = vp.request.modeValidation;
                        vm->request.modeValidation.overrides |= NVKMS_MODE_VALIDATION_ALLOW_NON_EDID_MODES;
                        vm->request.mode.timings = *t;
                        NvModeTimings *m = &vm->request.mode.timings;
                        m->vVisible -= trim; m->vSyncStart -= trim; m->vSyncEnd -= trim; m->vTotal -= trim;
                        m->pixelClockHz = (NvU64)((double)t->pixelClockHz * m->vTotal / t->vTotal);
                        m->pixelClockHz = (m->pixelClockHz / 2000) * 2000;
                        vm->request.infoStringSize = NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH;
                        vm->request.pInfoString = (NvU64)(NvUPtr)info;
                        memset(info, 0, NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH);
                        if (nvkms(NVKMS_IOCTL_VALIDATE_MODE, vm, sizeof(*vm))) { free(vm); break; }
                        char *last = strrchr(info, '\n');
                        if (last && !last[1]) { *last = 0; last = strrchr(info, '\n'); }
                        printf("SWEEP[%u] %ux%u pclk=%.3fMHz -> %s %s\n", mi, m->hVisible, m->vVisible,
                               m->pixelClockHz / 1e6, vm->reply.valid ? "VALID" : "INVALID",
                               vm->reply.valid ? "" : (last ? last + 1 : info));
                        free(vm);
                    }
                }
                if (hb_sync) {
                    /* Same visible size and refresh, horizontal blanking replaced (VALIDATE_MODE, no state change). */
                    struct NvKmsValidateModeParams *vm = calloc(1, sizeof(*vm));
                    vm->request.deviceHandle = dh; vm->request.dispHandle = dp; vm->request.dpyId = id;
                    vm->request.modeValidation = vp.request.modeValidation;
                    vm->request.modeValidation.overrides |= NVKMS_MODE_VALIDATION_ALLOW_NON_EDID_MODES;
                    vm->request.mode.timings = *t;
                    NvModeTimings *m = &vm->request.mode.timings;
                    m->hVisible = ha; m->vVisible = va;
                    m->vSyncStart = va + vfp; m->vSyncEnd = m->vSyncStart + vsw; m->vTotal = m->vSyncEnd + vbp;
                    m->hSyncStart = m->hVisible + hb_fp;
                    m->hSyncEnd = m->hSyncStart + hb_sync;
                    m->hTotal = m->hSyncEnd + hb_bp;
                    m->pixelClockHz = (NvU64)(hz * m->hTotal * m->vTotal);
                    m->pixelClockHz = (m->pixelClockHz / 10000) * 10000;
                    vm->request.infoStringSize = NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH;
                    vm->request.pInfoString = (NvU64)(NvUPtr)info;
                    memset(info, 0, NVKMS_MODE_VALIDATION_MAX_INFO_STRING_LENGTH);
                    if (!nvkms(NVKMS_IOCTL_VALIDATE_MODE, vm, sizeof(*vm))) {
                        char *last = strrchr(info, '\n');
                        if (last && !last[1]) { *last = 0; last = strrchr(info, '\n'); }
                        printf("HBLANK[%u] %ux%u htot=%u vtot=%u pclk=%.3fMHz refresh=%.4f -> %s %s\n", mi,
                               m->hVisible, m->vVisible, m->hTotal, m->vTotal, m->pixelClockHz / 1e6,
                               (double)m->pixelClockHz / ((double)m->hTotal * m->vTotal),
                               vm->reply.valid ? "VALID" : "INVALID", vm->reply.valid ? "" : (last ? last + 1 : info));
                    }
                    free(vm);
                }
                if (quiet_invalid && vp.reply.valid) continue;
                double rr = t->hTotal && t->vTotal ? (double)t->pixelClockHz / ((double)t->hTotal * t->vTotal) : 0;
                printf("MODE[%u] %ux%u@%.3f pclk=%.3fMHz htot=%u vtot=%u %s src=%d \"%s\" -> %s\n",
                       mi, t->hVisible, t->vVisible, rr, t->pixelClockHz / 1e6,
                       t->hTotal, t->vTotal, t->interlaced ? "interlaced" : "",
                       vp.reply.source, vp.reply.description,
                       vp.reply.valid ? "VALID" : "INVALID");
                printf("  infoString(%u bytes):\n%s%s", vp.reply.infoStringLenWritten, info,
                       (vp.reply.infoStringLenWritten && info[strlen(info) - 1] == '\n') ? "" : "\n");
            }
            free(info);
        }
    }

    struct NvKmsFreeDeviceParams fp = { .request = { dh } };
    nvkms(NVKMS_IOCTL_FREE_DEVICE, &fp, sizeof(fp));
    close(fd);
    return rc;
}
