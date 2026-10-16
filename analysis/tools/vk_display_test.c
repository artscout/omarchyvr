/* Vulkan direct-display test for the Pimax DP-1 lease, mirrors Monado's comp_window_direct_wayland.c +
 * comp_window_direct.c + comp_target_swapchain.c. Left half RED, right half GREEN (yellow square top-left of each half).
 * Usage: vk_display_test [key=value ...]     (all keys optional; defaults == Monado baseline)
 *   secs=25 hz=72 mode=<index>        display mode by refresh (default 7104x3840@hz) or by index
 *   present=fifo|immediate|mailbox|relaxed        (Monado default: fifo)
 *   plane=0|N|first                   display plane index (Monado: 0 without checking); 'first' = first plane supporting the display
 *   stack=current|N                   planeStackIndex (Monado: plane's currentStackIndex)
 *   alpha=auto|opaque|global|premul|perpixel   VkDisplaySurfaceCreateInfo alphaMode (Monado auto: premul > perpixel > global)
 *   galpha=1.0                        globalAlpha
 *   stransform=identity               surface create transform (identity|rot90|rot180|rot270)
 *   extent=mode|caps|WxH              swapchain imageExtent (Monado: caps.currentExtent; surface extent = mode size)
 *   sextent=mode|WxH                  VkDisplaySurfaceCreateInfo.imageExtent (Monado: mode size)
 *   format=auto|b8g8r8a8_srgb|b8g8r8a8_unorm|r8g8b8a8_unorm|r8g8b8a8_srgb|a2b10g10r10|a2r10g10b10|a8b8g8r8_unorm|a8b8g8r8_srgb
 *   colorspace=srgb|any               (Monado: SRGB_NONLINEAR first)
 *   images=N                          swapchain minImageCount (Monado: 2 or caps.min if larger)
 *   pretransform=current|identity     swapchain preTransform (Monado: currentTransform)
 *   alphac=auto|opaque|inherit        swapchain compositeAlpha (Monado: opaque, else inherit)
 *   clipped=1|0                       (Monado: 1)
 *   render=clear|copy                 clear = dynamic rendering clear + vkCmdClearAttachments (COLOR_ATTACHMENT usage, like Monado)
 *                                     copy  = buffer->image copy (TRANSFER_DST usage)
 *   pw=0|1                            use VK_KHR_present_id + present_wait and vkWaitForPresentKHR each frame
 *   nofw=0|1                          do not start pimax_video_on.py
 *   exclusive=0|1                     (printing only) sharing mode is always EXCLUSIVE
 * Build: see bottom comment. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include <unistd.h>
#include <time.h>
#include <wayland-client.h>
#include <vulkan/vulkan.h>
#include "drm-lease-v1-client-protocol.h"

/* ---------------- wayland lease (same as kms_lease_test.c, but keep the device drm fd) ---------------- */
static struct wp_drm_lease_device_v1 *dev;
static struct wp_drm_lease_connector_v1 *dp1;
static int lease_fd = -1, finished, dev_drm_fd = -1;
static struct { struct wp_drm_lease_connector_v1 *c; char name[64]; uint32_t id; } conns[16];
static int nconn;
static void d_fd(void *d, struct wp_drm_lease_device_v1 *x, int32_t fd) { dev_drm_fd = fd; }
static void d_conn(void *d, struct wp_drm_lease_device_v1 *x, struct wp_drm_lease_connector_v1 *c);
static void d_done(void *d, struct wp_drm_lease_device_v1 *x) {}
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

/* ---------------- options ---------------- */
static const char *opt(int argc, char **argv, const char *k, const char *def) {
    size_t n = strlen(k);
    for (int i = 1; i < argc; i++) if (!strncmp(argv[i], k, n) && argv[i][n] == '=') return argv[i] + n + 1;
    return def; }
#define OPT(k, d) opt(argc, argv, k, d)
#define VK(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "FAIL %s:%d %s = %d\n", __FILE__, __LINE__, #x, r_); exit(2); } } while (0)

static const char *pm_name(VkPresentModeKHR m) {
    switch (m) { case VK_PRESENT_MODE_IMMEDIATE_KHR: return "IMMEDIATE"; case VK_PRESENT_MODE_MAILBOX_KHR: return "MAILBOX";
    case VK_PRESENT_MODE_FIFO_KHR: return "FIFO"; case VK_PRESENT_MODE_FIFO_RELAXED_KHR: return "FIFO_RELAXED";
    case VK_PRESENT_MODE_SHARED_DEMAND_REFRESH_KHR: return "SHARED_DEMAND"; case VK_PRESENT_MODE_SHARED_CONTINUOUS_REFRESH_KHR: return "SHARED_CONT"; default: return "?"; } }
static const struct { const char *n; VkFormat f; } fmts[] = {
    {"b8g8r8a8_srgb", VK_FORMAT_B8G8R8A8_SRGB}, {"b8g8r8a8_unorm", VK_FORMAT_B8G8R8A8_UNORM},
    {"r8g8b8a8_srgb", VK_FORMAT_R8G8B8A8_SRGB}, {"r8g8b8a8_unorm", VK_FORMAT_R8G8B8A8_UNORM},
    {"a8b8g8r8_srgb", VK_FORMAT_A8B8G8R8_SRGB_PACK32}, {"a8b8g8r8_unorm", VK_FORMAT_A8B8G8R8_UNORM_PACK32},
    {"a2b10g10r10", VK_FORMAT_A2B10G10R10_UNORM_PACK32}, {"a2r10g10b10", VK_FORMAT_A2R10G10B10_UNORM_PACK32},
    {"r16g16b16a16_sfloat", VK_FORMAT_R16G16B16A16_SFLOAT} };
static const char *fmt_name(VkFormat f) { for (unsigned i = 0; i < sizeof fmts / sizeof *fmts; i++) if (fmts[i].f == f) return fmts[i].n; return "other"; }

/* copy-mode pixel packing */
static uint32_t pack(VkFormat f, int r, int g, int b) {
    switch (f) {
    case VK_FORMAT_B8G8R8A8_UNORM: case VK_FORMAT_B8G8R8A8_SRGB: return 0xff000000u | r << 16 | g << 8 | b;
    case VK_FORMAT_R8G8B8A8_UNORM: case VK_FORMAT_R8G8B8A8_SRGB:
    case VK_FORMAT_A8B8G8R8_UNORM_PACK32: case VK_FORMAT_A8B8G8R8_SRGB_PACK32: return 0xff000000u | b << 16 | g << 8 | r;
    case VK_FORMAT_A2B10G10R10_UNORM_PACK32: return 3u << 30 | (b ? 1023u : 0) << 20 | (g ? 1023u : 0) << 10 | (r ? 1023u : 0);
    case VK_FORMAT_A2R10G10B10_UNORM_PACK32: return 3u << 30 | (r ? 1023u : 0) << 20 | (g ? 1023u : 0) << 10 | (b ? 1023u : 0);
    default: return 0; } }

static uint32_t find_mem(VkPhysicalDevice pd, uint32_t bits, VkMemoryPropertyFlags want) {
    VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(pd, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) if ((bits & (1u << i)) && (mp.memoryTypes[i].propertyFlags & want) == want) return i;
    fprintf(stderr, "no mem type\n"); exit(2); }

int main(int argc, char **argv) {
    int secs = atoi(OPT("secs", "25")), hz = atoi(OPT("hz", "72"));
    const char *s_present = OPT("present", "fifo"), *s_plane = OPT("plane", "0"), *s_stack = OPT("stack", "current"),
        *s_alpha = OPT("alpha", "auto"), *s_extent = OPT("extent", "caps"), *s_sextent = OPT("sextent", "mode"),
        *s_format = OPT("format", "auto"), *s_cs = OPT("colorspace", "srgb"), *s_pre = OPT("pretransform", "current"),
        *s_alphac = OPT("alphac", "auto"), *s_render = OPT("render", "clear"), *s_st = OPT("stransform", "identity");
    float galpha = (float)atof(OPT("galpha", "1.0"));
    int images = atoi(OPT("images", "0")), clipped = atoi(OPT("clipped", "1")), pw = atoi(OPT("pw", "0")), nofw = atoi(OPT("nofw", "0"));
    const char *s_mode = OPT("mode", NULL);
    bool copy = !strcmp(s_render, "copy");

    /* ---- lease ---- */
    struct wl_display *dpy = wl_display_connect(NULL);
    if (!dpy) { fprintf(stderr, "no wayland display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(dpy);
    wl_registry_add_listener(reg, &rl, NULL);
    wl_display_roundtrip(dpy);
    if (!dev) { fprintf(stderr, "no wp_drm_lease_device_v1\n"); return 1; }
    wl_display_roundtrip(dpy); wl_display_roundtrip(dpy);
    uint32_t conn_id = 0;
    for (int i = 0; i < nconn; i++) { printf("lease connector: %s id=%u\n", conns[i].name, conns[i].id);
        if (!strcmp(conns[i].name, "DP-1")) { dp1 = conns[i].c; conn_id = conns[i].id; } }
    if (!dp1) { fprintf(stderr, "DP-1 not offered\n"); return 1; }
    struct wp_drm_lease_request_v1 *rq = wp_drm_lease_device_v1_create_lease_request(dev);
    wp_drm_lease_request_v1_request_connector(rq, dp1);
    struct wp_drm_lease_v1 *lease = wp_drm_lease_request_v1_submit(rq);
    wp_drm_lease_v1_add_listener(lease, &ll, NULL);
    while (lease_fd < 0 && !finished) if (wl_display_roundtrip(dpy) < 0) break;
    if (lease_fd < 0) { fprintf(stderr, "LEASE FAILED\n"); return 1; }
    printf("LEASE OK leased_fd=%d device_drm_fd=%d connector=%u\n", lease_fd, dev_drm_fd, conn_id);

    /* ---- instance (Monado: VK_KHR_display, direct_mode_display, acquire_drm_display) ---- */
    const char *iexts[] = { VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_DISPLAY_EXTENSION_NAME, VK_EXT_DIRECT_MODE_DISPLAY_EXTENSION_NAME,
        VK_EXT_ACQUIRE_DRM_DISPLAY_EXTENSION_NAME, VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME };
    VkApplicationInfo ai = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .pApplicationName = "vk_display_test", .apiVersion = VK_API_VERSION_1_3 };
    VkInstanceCreateInfo ici = { .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &ai,
        .enabledExtensionCount = 5, .ppEnabledExtensionNames = iexts };
    VkInstance inst; VK(vkCreateInstance(&ici, NULL, &inst));
#define IPFN(n) PFN_##n n = (PFN_##n)vkGetInstanceProcAddr(inst, #n); if (!n) { fprintf(stderr, "missing " #n "\n"); return 2; }
    IPFN(vkGetDrmDisplayEXT) IPFN(vkAcquireDrmDisplayEXT) IPFN(vkReleaseDisplayEXT)
    IPFN(vkGetDisplayModePropertiesKHR) IPFN(vkGetPhysicalDeviceDisplayPlanePropertiesKHR) IPFN(vkGetDisplayPlaneSupportedDisplaysKHR)
    IPFN(vkGetDisplayPlaneCapabilitiesKHR) IPFN(vkCreateDisplayPlaneSurfaceKHR) IPFN(vkDestroySurfaceKHR)
    IPFN(vkGetPhysicalDeviceSurfaceCapabilitiesKHR) IPFN(vkGetPhysicalDeviceSurfaceFormatsKHR) IPFN(vkGetPhysicalDeviceSurfacePresentModesKHR)
    IPFN(vkGetPhysicalDeviceSurfaceSupportKHR)

    uint32_t npd = 0; vkEnumeratePhysicalDevices(inst, &npd, NULL);
    VkPhysicalDevice pds[8]; if (npd > 8) npd = 8; vkEnumeratePhysicalDevices(inst, &npd, pds);
    VkPhysicalDevice pd = VK_NULL_HANDLE; VkDisplayKHR display = VK_NULL_HANDLE;
    for (uint32_t i = 0; i < npd; i++) {
        VkPhysicalDeviceProperties pp; vkGetPhysicalDeviceProperties(pds[i], &pp);
        VkResult r = vkGetDrmDisplayEXT(pds[i], dev_drm_fd, conn_id, &display);
        printf("physical device %u: %s -> vkGetDrmDisplayEXT = %d\n", i, pp.deviceName, r);
        if (r == VK_SUCCESS) { pd = pds[i]; break; } }
    if (!pd) { fprintf(stderr, "vkGetDrmDisplayEXT failed on all devices\n"); return 2; }
    VkResult ar = vkAcquireDrmDisplayEXT(pd, lease_fd, display);
    printf("vkAcquireDrmDisplayEXT(leased_fd) = %d\n", ar);
    if (ar != VK_SUCCESS) return 2;

    /* ---- display mode ---- */
    uint32_t nm = 0; vkGetDisplayModePropertiesKHR(pd, display, &nm, NULL);
    VkDisplayModePropertiesKHR *mp = calloc(nm, sizeof *mp);
    VK(vkGetDisplayModePropertiesKHR(pd, display, &nm, mp));
    int mi = -1;
    for (uint32_t i = 0; i < nm; i++) {
        VkDisplayModeParametersKHR *p = &mp[i].parameters;
        printf("  display mode %u: %ux%u@%.3f\n", i, p->visibleRegion.width, p->visibleRegion.height, p->refreshRate / 1000.0);
        if (mi < 0 && p->visibleRegion.width == 7104 && p->visibleRegion.height == 3840 && (p->refreshRate + 500) / 1000 == (uint32_t)hz) mi = i; }
    if (s_mode) mi = atoi(s_mode);
    if (mi < 0 || mi >= (int)nm) { fprintf(stderr, "mode not found\n"); return 2; }
    VkDisplayModeKHR dmode = mp[mi].displayMode;
    uint32_t mw = mp[mi].parameters.visibleRegion.width, mh = mp[mi].parameters.visibleRegion.height;
    printf("CHOSEN mode %d: %ux%u@%.3f\n", mi, mw, mh, mp[mi].parameters.refreshRate / 1000.0);

    /* ---- planes ---- */
    uint32_t npl = 0; vkGetPhysicalDeviceDisplayPlanePropertiesKHR(pd, &npl, NULL);
    VkDisplayPlanePropertiesKHR *plp = calloc(npl, sizeof *plp);
    VK(vkGetPhysicalDeviceDisplayPlanePropertiesKHR(pd, &npl, plp));
    int first_supp = -1;
    for (uint32_t i = 0; i < npl; i++) {
        uint32_t nd = 0; vkGetDisplayPlaneSupportedDisplaysKHR(pd, i, &nd, NULL);
        VkDisplayKHR *ds = calloc(nd ? nd : 1, sizeof *ds); vkGetDisplayPlaneSupportedDisplaysKHR(pd, i, &nd, ds);
        bool sup = false; for (uint32_t k = 0; k < nd; k++) if (ds[k] == display) sup = true;
        free(ds);
        printf("  plane %u: currentDisplay=%p currentStackIndex=%u supportsOurDisplay=%d\n", i, (void *)plp[i].currentDisplay, plp[i].currentStackIndex, sup);
        if (sup && first_supp < 0) first_supp = i; }
    uint32_t plane = !strcmp(s_plane, "first") ? (first_supp < 0 ? 0 : first_supp) : (uint32_t)atoi(s_plane);
    if (plane >= npl) { fprintf(stderr, "plane %u out of range\n", plane); return 2; }
    uint32_t stack = !strcmp(s_stack, "current") ? plp[plane].currentStackIndex : (uint32_t)atoi(s_stack);

    VkDisplayPlaneCapabilitiesKHR pc; VK(vkGetDisplayPlaneCapabilitiesKHR(pd, dmode, plane, &pc));
    printf("plane %u caps: supportedAlpha=0x%x src[%ux%u..%ux%u] dst[%ux%u..%ux%u]\n", plane, pc.supportedAlpha,
        pc.minSrcExtent.width, pc.minSrcExtent.height, pc.maxSrcExtent.width, pc.maxSrcExtent.height,
        pc.minDstExtent.width, pc.minDstExtent.height, pc.maxDstExtent.width, pc.maxDstExtent.height);
    VkDisplayPlaneAlphaFlagBitsKHR am;
    if (!strcmp(s_alpha, "opaque")) am = VK_DISPLAY_PLANE_ALPHA_OPAQUE_BIT_KHR;
    else if (!strcmp(s_alpha, "global")) am = VK_DISPLAY_PLANE_ALPHA_GLOBAL_BIT_KHR;
    else if (!strcmp(s_alpha, "premul")) am = VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_PREMULTIPLIED_BIT_KHR;
    else if (!strcmp(s_alpha, "perpixel")) am = VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_BIT_KHR;
    else if (pc.supportedAlpha & VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_PREMULTIPLIED_BIT_KHR) am = VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_PREMULTIPLIED_BIT_KHR;
    else if (pc.supportedAlpha & VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_BIT_KHR) am = VK_DISPLAY_PLANE_ALPHA_PER_PIXEL_BIT_KHR;
    else am = VK_DISPLAY_PLANE_ALPHA_GLOBAL_BIT_KHR;
    VkSurfaceTransformFlagBitsKHR st = !strcmp(s_st, "rot90") ? VK_SURFACE_TRANSFORM_ROTATE_90_BIT_KHR :
        !strcmp(s_st, "rot180") ? VK_SURFACE_TRANSFORM_ROTATE_180_BIT_KHR : !strcmp(s_st, "rot270") ? VK_SURFACE_TRANSFORM_ROTATE_270_BIT_KHR : VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
    uint32_t sw = mw, sh = mh;
    if (strcmp(s_sextent, "mode")) sscanf(s_sextent, "%ux%u", &sw, &sh);
    VkDisplaySurfaceCreateInfoKHR sci = { .sType = VK_STRUCTURE_TYPE_DISPLAY_SURFACE_CREATE_INFO_KHR, .displayMode = dmode, .planeIndex = plane,
        .planeStackIndex = stack, .transform = st, .globalAlpha = galpha, .alphaMode = am, .imageExtent = { sw, sh } };
    printf("SURFACE: plane=%u stack=%u alphaMode=0x%x globalAlpha=%.2f transform=0x%x imageExtent=%ux%u\n", plane, stack, am, galpha, st, sw, sh);
    VkSurfaceKHR surf; VK(vkCreateDisplayPlaneSurfaceKHR(inst, &sci, NULL, &surf));

    /* ---- device ---- */
    uint32_t nq = 0; vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, NULL);
    VkQueueFamilyProperties qf[16]; if (nq > 16) nq = 16; vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, qf);
    int qfi = -1;
    for (uint32_t i = 0; i < nq; i++) { VkBool32 s = 0; vkGetPhysicalDeviceSurfaceSupportKHR(pd, i, surf, &s);
        printf("  queue family %u flags=0x%x present=%u\n", i, qf[i].queueFlags, s);
        if (qfi < 0 && s && (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) qfi = i; }
    if (qfi < 0) { fprintf(stderr, "no graphics+present queue\n"); return 2; }
    const char *dexts[3] = { VK_KHR_SWAPCHAIN_EXTENSION_NAME }; int ndx = 1;
    VkPhysicalDevicePresentIdFeaturesKHR pidf = { .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PRESENT_ID_FEATURES_KHR, .presentId = VK_TRUE };
    VkPhysicalDevicePresentWaitFeaturesKHR pwf = { .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PRESENT_WAIT_FEATURES_KHR, .pNext = &pidf, .presentWait = VK_TRUE };
    VkPhysicalDeviceVulkan13Features f13 = { .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES, .dynamicRendering = VK_TRUE, .synchronization2 = VK_TRUE };
    if (pw) { dexts[ndx++] = VK_KHR_PRESENT_ID_EXTENSION_NAME; dexts[ndx++] = VK_KHR_PRESENT_WAIT_EXTENSION_NAME; f13.pNext = &pwf; }
    float prio = 1.0f;
    VkDeviceQueueCreateInfo qci = { .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueFamilyIndex = qfi, .queueCount = 1, .pQueuePriorities = &prio };
    VkDeviceCreateInfo dci = { .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .pNext = &f13, .queueCreateInfoCount = 1, .pQueueCreateInfos = &qci,
        .enabledExtensionCount = ndx, .ppEnabledExtensionNames = dexts };
    VkDevice dv; VK(vkCreateDevice(pd, &dci, NULL, &dv));
    VkQueue q; vkGetDeviceQueue(dv, qfi, 0, &q);
#define DPFN(n) PFN_##n n = (PFN_##n)vkGetDeviceProcAddr(dv, #n);
    DPFN(vkCreateSwapchainKHR) DPFN(vkDestroySwapchainKHR) DPFN(vkGetSwapchainImagesKHR) DPFN(vkAcquireNextImageKHR) DPFN(vkQueuePresentKHR)
    DPFN(vkCmdBeginRendering) DPFN(vkCmdEndRendering) DPFN(vkCmdPipelineBarrier2) DPFN(vkWaitForPresentKHR)

    /* ---- swapchain (comp_target_swapchain.c choices) ---- */
    VkSurfaceCapabilitiesKHR caps; VK(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(pd, surf, &caps));
    printf("surface caps: minImages=%u maxImages=%u currentExtent=%ux%u min=%ux%u max=%ux%u currentTransform=0x%x supportedTransforms=0x%x compositeAlpha=0x%x usage=0x%x\n",
        caps.minImageCount, caps.maxImageCount, caps.currentExtent.width, caps.currentExtent.height, caps.minImageExtent.width, caps.minImageExtent.height,
        caps.maxImageExtent.width, caps.maxImageExtent.height, caps.currentTransform, caps.supportedTransforms, caps.supportedCompositeAlpha, caps.supportedUsageFlags);
    uint32_t npm = 0; vkGetPhysicalDeviceSurfacePresentModesKHR(pd, surf, &npm, NULL);
    VkPresentModeKHR pms[16]; if (npm > 16) npm = 16; vkGetPhysicalDeviceSurfacePresentModesKHR(pd, surf, &npm, pms);
    printf("present modes:"); for (uint32_t i = 0; i < npm; i++) printf(" %s", pm_name(pms[i])); printf("\n");
    VkPresentModeKHR pmode = !strcmp(s_present, "immediate") ? VK_PRESENT_MODE_IMMEDIATE_KHR : !strcmp(s_present, "mailbox") ? VK_PRESENT_MODE_MAILBOX_KHR :
        !strcmp(s_present, "relaxed") ? VK_PRESENT_MODE_FIFO_RELAXED_KHR : VK_PRESENT_MODE_FIFO_KHR;
    bool pm_ok = false; for (uint32_t i = 0; i < npm; i++) if (pms[i] == pmode) pm_ok = true;
    if (!pm_ok) { fprintf(stderr, "present mode %s unsupported\n", pm_name(pmode)); return 2; }

    uint32_t nf = 0; vkGetPhysicalDeviceSurfaceFormatsKHR(pd, surf, &nf, NULL);
    VkSurfaceFormatKHR *sf = calloc(nf, sizeof *sf); vkGetPhysicalDeviceSurfaceFormatsKHR(pd, surf, &nf, sf);
    printf("surface formats:"); for (uint32_t i = 0; i < nf; i++) printf(" %s(%d,cs%d)", fmt_name(sf[i].format), sf[i].format, sf[i].colorSpace); printf("\n");
    VkSurfaceFormatKHR fmt = {0}; bool got = false;
    VkFormat monado_order[] = { VK_FORMAT_B8G8R8A8_SRGB, VK_FORMAT_R8G8B8A8_SRGB, VK_FORMAT_A8B8G8R8_SRGB_PACK32 };
    VkFormat want[3]; int nwant = 3; memcpy(want, monado_order, sizeof want);
    if (strcmp(s_format, "auto")) { nwant = 0; for (unsigned i = 0; i < sizeof fmts / sizeof *fmts; i++) if (!strcmp(s_format, fmts[i].n)) want[nwant++] = fmts[i].f;
        if (!nwant) { fprintf(stderr, "unknown format %s\n", s_format); return 2; } }
    for (int pass = 0; pass < 2 && !got; pass++) for (int w = 0; w < nwant && !got; w++) for (uint32_t i = 0; i < nf && !got; i++)
        if (sf[i].format == want[w] && (pass == 1 || !strcmp(s_cs, "any") || sf[i].colorSpace == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR)) { fmt = sf[i]; got = true; }
    if (!got) { fprintf(stderr, "requested format not available\n"); return 2; }
    if (copy && pack(fmt.format, 1, 0, 0) == 0) { fprintf(stderr, "render=copy unsupported for this format\n"); return 2; }

    VkExtent2D ext = caps.currentExtent;
    if (!strcmp(s_extent, "mode")) { ext.width = mw; ext.height = mh; }
    else if (strcmp(s_extent, "caps")) sscanf(s_extent, "%ux%u", &ext.width, &ext.height);
    if (ext.width == 0xffffffffu) { ext.width = mw; ext.height = mh; }
    uint32_t nimg = images;
    if (!nimg) nimg = caps.minImageCount >= 2 ? caps.minImageCount : (caps.maxImageCount == 0 || caps.maxImageCount >= 2 ? 2 : caps.maxImageCount);
    VkCompositeAlphaFlagBitsKHR ca = !strcmp(s_alphac, "inherit") ? VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR :
        !strcmp(s_alphac, "opaque") ? VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR :
        (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR) ? VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR : VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    VkImageUsageFlags usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | (copy ? VK_IMAGE_USAGE_TRANSFER_DST_BIT : 0);
    VkSurfaceTransformFlagBitsKHR pre = !strcmp(s_pre, "identity") ? VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR : caps.currentTransform;
    VkSwapchainCreateInfoKHR sc = { .sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR, .surface = surf, .minImageCount = nimg, .imageFormat = fmt.format,
        .imageColorSpace = fmt.colorSpace, .imageExtent = ext, .imageArrayLayers = 1, .imageUsage = usage, .imageSharingMode = VK_SHARING_MODE_EXCLUSIVE,
        .preTransform = pre, .compositeAlpha = ca, .presentMode = pmode, .clipped = clipped ? VK_TRUE : VK_FALSE };
    printf("SWAPCHAIN: format=%s(%d) colorSpace=%d extent=%ux%u minImageCount=%u usage=0x%x sharing=EXCLUSIVE preTransform=0x%x compositeAlpha=0x%x presentMode=%s clipped=%d render=%s presentWait=%d\n",
        fmt_name(fmt.format), fmt.format, fmt.colorSpace, ext.width, ext.height, nimg, usage, pre, ca, pm_name(pmode), clipped, s_render, pw);
    VkSwapchainKHR sw_; VK(vkCreateSwapchainKHR(dv, &sc, NULL, &sw_));
    uint32_t ni = 0; vkGetSwapchainImagesKHR(dv, sw_, &ni, NULL);
    VkImage *img = calloc(ni, sizeof *img); vkGetSwapchainImagesKHR(dv, sw_, &ni, img);
    VkImageView *iv = calloc(ni, sizeof *iv);
    for (uint32_t i = 0; i < ni; i++) {
        VkImageViewCreateInfo vi = { .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO, .image = img[i], .viewType = VK_IMAGE_VIEW_TYPE_2D, .format = fmt.format,
            .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 } };
        if (!copy) VK(vkCreateImageView(dv, &vi, NULL, &iv[i])); }
    printf("swapchain created, %u images\n", ni);

    /* ---- copy source buffer ---- */
    VkBuffer buf = VK_NULL_HANDLE; VkDeviceMemory bmem = VK_NULL_HANDLE;
    if (copy) {
        VkDeviceSize sz = (VkDeviceSize)ext.width * ext.height * 4;
        VkBufferCreateInfo bi = { .sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .size = sz, .usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT, .sharingMode = VK_SHARING_MODE_EXCLUSIVE };
        VK(vkCreateBuffer(dv, &bi, NULL, &buf));
        VkMemoryRequirements mr; vkGetBufferMemoryRequirements(dv, buf, &mr);
        VkMemoryAllocateInfo mai = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = mr.size,
            .memoryTypeIndex = find_mem(pd, mr.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) };
        VK(vkAllocateMemory(dv, &mai, NULL, &bmem)); VK(vkBindBufferMemory(dv, buf, bmem, 0));
        uint32_t *p; VK(vkMapMemory(dv, bmem, 0, sz, 0, (void **)&p));
        uint32_t R = pack(fmt.format, 1, 0, 0), G = pack(fmt.format, 0, 1, 0), Y = pack(fmt.format, 1, 1, 0);
        for (uint32_t y = 0; y < ext.height; y++) for (uint32_t x = 0; x < ext.width; x++) {
            bool right = x >= ext.width / 2; uint32_t lx = right ? x - ext.width / 2 : x;
            p[(size_t)y * ext.width + x] = (lx < 400 && y < 400) ? Y : right ? G : R; }
        vkUnmapMemory(dv, bmem);
    }

    /* ---- sync / cmd ---- */
    VkCommandPool cp; VkCommandPoolCreateInfo cpi = { .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT, .queueFamilyIndex = qfi };
    VK(vkCreateCommandPool(dv, &cpi, NULL, &cp));
    VkCommandBuffer cb; VkCommandBufferAllocateInfo cbi = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = cp, .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = 1 };
    VK(vkAllocateCommandBuffers(dv, &cbi, &cb));
    VkSemaphoreCreateInfo semi = { .sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO };
    VkSemaphore acq; VK(vkCreateSemaphore(dv, &semi, NULL, &acq));
    VkSemaphore *rel = calloc(ni, sizeof *rel); for (uint32_t i = 0; i < ni; i++) VK(vkCreateSemaphore(dv, &semi, NULL, &rel[i]));
    VkFence fence; VkFenceCreateInfo fi = { .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO, .flags = VK_FENCE_CREATE_SIGNALED_BIT };
    VK(vkCreateFence(dv, &fi, NULL, &fence));

    if (!nofw) {
        system("python3 /home/user/pimax-re/linux/pimax_video_on.py 72 40 > /tmp/vk-test-fwlog.txt 2>&1 &");
        printf("started pimax_video_on.py (log /tmp/vk-test-fwlog.txt)\n");
        sleep(2);
    }
    fflush(stdout);

    struct timespec t0; clock_gettime(CLOCK_MONOTONIC, &t0);
    uint64_t frame = 0, pid = 0; int rc = 0;
    for (;;) {
        struct timespec t1; clock_gettime(CLOCK_MONOTONIC, &t1);
        double el = (t1.tv_sec - t0.tv_sec) + (t1.tv_nsec - t0.tv_nsec) / 1e9;
        if (el >= secs) break;
        wl_display_dispatch_pending(dpy); wl_display_flush(dpy);
        vkWaitForFences(dv, 1, &fence, VK_TRUE, UINT64_MAX); vkResetFences(dv, 1, &fence);
        uint32_t ix; VkResult r = vkAcquireNextImageKHR(dv, sw_, 1000000000ull, acq, VK_NULL_HANDLE, &ix);
        if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) { fprintf(stderr, "acquire failed %d at frame %lu\n", r, (unsigned long)frame); rc = 3; break; }
        vkResetCommandBuffer(cb, 0);
        VkCommandBufferBeginInfo bi = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO, .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT };
        vkBeginCommandBuffer(cb, &bi);
        VkImageSubresourceRange rng = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 };
        VkImageMemoryBarrier2 b1 = { .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2, .srcStageMask = VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT, .dstStageMask = VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
            .dstAccessMask = VK_ACCESS_2_MEMORY_WRITE_BIT, .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED, .newLayout = copy ? VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL : VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
            .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .image = img[ix], .subresourceRange = rng };
        VkDependencyInfo di = { .sType = VK_STRUCTURE_TYPE_DEPENDENCY_INFO, .imageMemoryBarrierCount = 1, .pImageMemoryBarriers = &b1 };
        vkCmdPipelineBarrier2(cb, &di);
        if (copy) {
            VkBufferImageCopy reg_ = { .imageSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 }, .imageExtent = { ext.width, ext.height, 1 } };
            vkCmdCopyBufferToImage(cb, buf, img[ix], VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &reg_);
        } else {
            VkRenderingAttachmentInfo att = { .sType = VK_STRUCTURE_TYPE_RENDERING_ATTACHMENT_INFO, .imageView = iv[ix], .imageLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
                .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR, .storeOp = VK_ATTACHMENT_STORE_OP_STORE, .clearValue = { .color = { .float32 = { 1, 0, 0, 1 } } } };
            VkRenderingInfo ri = { .sType = VK_STRUCTURE_TYPE_RENDERING_INFO, .renderArea = { {0, 0}, ext }, .layerCount = 1, .colorAttachmentCount = 1, .pColorAttachments = &att };
            vkCmdBeginRendering(cb, &ri);
            uint32_t hw = ext.width / 2;
            VkClearAttachment ca_ = { VK_IMAGE_ASPECT_COLOR_BIT, 0, { .color = { .float32 = { 0, 1, 0, 1 } } } };
            VkClearRect cr = { { { (int32_t)hw, 0 }, { ext.width - hw, ext.height } }, 0, 1 };
            vkCmdClearAttachments(cb, 1, &ca_, 1, &cr);
            VkClearAttachment cy = { VK_IMAGE_ASPECT_COLOR_BIT, 0, { .color = { .float32 = { 1, 1, 0, 1 } } } };
            VkClearRect cy2[2] = { { { { 0, 0 }, { 400, 400 } }, 0, 1 }, { { { (int32_t)hw, 0 }, { 400, 400 } }, 0, 1 } };
            vkCmdClearAttachments(cb, 1, &cy, 2, cy2);
            vkCmdEndRendering(cb);
        }
        VkImageMemoryBarrier2 b2 = b1; b2.srcAccessMask = VK_ACCESS_2_MEMORY_WRITE_BIT; b2.dstAccessMask = 0;
        b2.oldLayout = b1.newLayout; b2.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
        di.pImageMemoryBarriers = &b2; vkCmdPipelineBarrier2(cb, &di);
        vkEndCommandBuffer(cb);
        VkPipelineStageFlags ws = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        VkSubmitInfo si = { .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO, .waitSemaphoreCount = 1, .pWaitSemaphores = &acq, .pWaitDstStageMask = &ws,
            .commandBufferCount = 1, .pCommandBuffers = &cb, .signalSemaphoreCount = 1, .pSignalSemaphores = &rel[ix] };
        VK(vkQueueSubmit(q, 1, &si, fence));
        VkPresentIdKHR pidi = { .sType = VK_STRUCTURE_TYPE_PRESENT_ID_KHR, .swapchainCount = 1, .pPresentIds = &(uint64_t){ ++pid } };
        VkPresentInfoKHR pi = { .sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR, .pNext = pw ? &pidi : NULL, .waitSemaphoreCount = 1, .pWaitSemaphores = &rel[ix],
            .swapchainCount = 1, .pSwapchains = &sw_, .pImageIndices = &ix };
        r = vkQueuePresentKHR(q, &pi);
        if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) { fprintf(stderr, "present failed %d at frame %lu\n", r, (unsigned long)frame); rc = 3; break; }
        if (frame == 0) { printf("FIRST PRESENT OK (result %d)\n", r); fflush(stdout); }
        if (pw) { VkResult w = vkWaitForPresentKHR(dv, sw_, pid, 1000000000ull); if (w != VK_SUCCESS) { fprintf(stderr, "wait_for_present %d\n", w); pw = 0; } }
        frame++;
    }
    struct timespec t2; clock_gettime(CLOCK_MONOTONIC, &t2);
    double tot = (t2.tv_sec - t0.tv_sec) + (t2.tv_nsec - t0.tv_nsec) / 1e9;
    printf("presented %lu frames in %.1f s (%.1f fps)\n", (unsigned long)frame, tot, frame / tot);

    vkDeviceWaitIdle(dv);
    vkDestroyFence(dv, fence, NULL); vkDestroySemaphore(dv, acq, NULL);
    for (uint32_t i = 0; i < ni; i++) { vkDestroySemaphore(dv, rel[i], NULL); if (iv[i]) vkDestroyImageView(dv, iv[i], NULL); }
    vkDestroyCommandPool(dv, cp, NULL);
    if (buf) { vkDestroyBuffer(dv, buf, NULL); vkFreeMemory(dv, bmem, NULL); }
    vkDestroySwapchainKHR(dv, sw_, NULL);
    vkDestroyDevice(dv, NULL);
    vkDestroySurfaceKHR(inst, surf, NULL);
    vkReleaseDisplayEXT(pd, display);
    vkDestroyInstance(inst, NULL);
    wp_drm_lease_v1_destroy(lease);
    wl_display_roundtrip(dpy);
    close(lease_fd);
    wl_display_disconnect(dpy);
    printf("lease released, exit\n");
    return rc;
}
/* Build (from this dir; drm-lease-v1-protocol.c / .h already generated here, see kms_lease_test.c):
 *  gcc -O2 -I. -o vk_display_test vk_display_test.c drm-lease-v1-protocol.c $(pkg-config --cflags --libs wayland-client) -lvulkan
 */
