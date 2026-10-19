// Lens tuning client: head-locked square grid + centre cross + circles at 10..50 deg (see draw_grid).
// CPU renders each eye once (first frame, with the FOV the runtime reports), copied to the swapchain per frame.
// Gaze markers (eye tracking check), drawn on top every frame:
//   cyan dot      = this eye's own gaze ray from `omarchyvr eyed` (/dev/shm/omarchyvr-eye-gaze, OMARCHYVR_EYE_GAZE_SHM)
//   magenta ring  = the runtime's OpenXR XR_EXT_eye_gaze_interaction gaze (through SteamVR / Monado), projected
//                   into each eye from the grid distance
// The two should sit on each other; both should sit where you look (the circles are 10..50 deg from the centre).
// Build: gcc -O2 -o xr_grid xr_grid.c -lopenxr_loader -lvulkan -lm -ldl
// Run:   XR_GRID_SECONDS=60 [XR_GRID_DIST=2.0] [XR_GRID_GAZE=0] ./xr_grid
#define XR_USE_GRAPHICS_API_VULKAN
#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <math.h>
#include <fcntl.h>
#include <unistd.h>

#define XC(x) do { XrResult r_ = (x); if (XR_FAILED(r_)) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)
#define VC(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)

static PFN_vkGetInstanceProcAddr gipa;
#define VKI(inst, name) PFN_##name name = (PFN_##name)gipa(inst, #name)

static void rect(uint32_t *px, int W, int H, int x0, int y0, int x1, int y1, uint32_t c)
{
	if (x0 < 0) x0 = 0; if (y0 < 0) y0 = 0; if (x1 > W) x1 = W; if (y1 > H) y1 = H;
	for (int y = y0; y < y1; y++) for (int x = x0; x < x1; x++) px[(size_t)y * W + x] = c;
}

// colours as 0xAABBGGRR (R8G8B8A8 byte order); swapped for BGRA formats afterwards.
// Head-locked "wall" at distance D: square grid every 0.2 m (bright every 5th, 1 m), thick centre cross, and
// concentric circles at 10/20/30/40/50 deg from the view axis (white, yellow, green, cyan, magenta). All drawn by
// ray-casting each pixel through the FOV the runtime reports, so straight lines / round circles mean the lens
// model matches the optics. Anti-aliased with the pixel footprint in tangent space.
static float cover(float d, float w, float pix) // AA coverage of a line of half-width w at distance d
{
	float c = (w - fabsf(d)) / pix + 0.5f;
	return c < 0 ? 0 : c > 1 ? 1 : c;
}
static uint32_t mix(uint32_t base, uint32_t col, float a)
{
	if (a <= 0) return base; if (a > 1) a = 1;
	uint32_t o = 0xFF000000;
	for (int sft = 0; sft <= 16; sft += 8) {
		float b = (base >> sft) & 0xFF, c = (col >> sft) & 0xFF;
		o |= (uint32_t)(b + (c - b) * a) << sft;
	}
	return o;
}
static void draw_grid(uint32_t *px, int W, int H, int eye, int bgra, XrFovf fov, float D)
{
	float tl = tanf(-fov.angleLeft), tr = tanf(fov.angleRight), tu = tanf(fov.angleUp), td = tanf(-fov.angleDown);
	float pix = (tl + tr) / W; // tangent-space size of one pixel
	static const float deg[5] = {10, 20, 30, 40, 50};
	static const uint32_t rc[5] = {0xFFFFFFFF, 0xFF00FFFF, 0xFF00FF00, 0xFFFFFF00, 0xFFFF00FF};
	float rt[5]; for (int i = 0; i < 5; i++) rt[i] = tanf(deg[i] * 3.14159265f / 180.0f);
	const float step = 0.2f / D; // grid spacing in tangent units on the wall
	uint32_t bg = eye == 0 ? 0xFF301808 : 0xFF080818;
	for (int y = 0; y < H; y++) {
		float ty = tu - (y + 0.5f) / H * (tu + td);
		for (int x = 0; x < W; x++) {
			float tx = -tl + (x + 0.5f) / W * (tl + tr);
			uint32_t c = bg;
			float gx = tx / step, gy = ty / step; // grid coordinate, integer = line
			float fx = gx - floorf(gx + 0.5f), fy = gy - floorf(gy + 0.5f);
			int ix = (int)floorf(gx + 0.5f), iy = (int)floorf(gy + 0.5f);
			float ps = pix / step; // one pixel in grid units
			float ax = cover(fx, ps * (ix % 5 == 0 ? 2.0f : 1.0f), ps), ay = cover(fy, ps * (iy % 5 == 0 ? 2.0f : 1.0f), ps);
			c = mix(c, 0xFF909090, ax); c = mix(c, 0xFF909090, ay);
			if (ix % 5 == 0) c = mix(c, 0xFFE0E0E0, ax);
			if (iy % 5 == 0) c = mix(c, 0xFFE0E0E0, ay);
			float r = sqrtf(tx * tx + ty * ty);
			for (int i = 0; i < 5; i++) c = mix(c, rc[i], cover(r - rt[i], 2.0f * pix, pix));
			c = mix(c, 0xFF0000FF, cover(tx, 3.0f * pix, pix)); // centre cross: red vertical + horizontal
			c = mix(c, 0xFF0000FF, cover(ty, 3.0f * pix, pix));
			// FOV border in orange so the visible edge of the image is obvious
			if (x < 4 || y < 4 || x >= W - 4 || y >= H - 4) c = 0xFF0080FF;
			px[(size_t)y * W + x] = c;
		}
	}
	if (bgra) for (size_t i = 0; i < (size_t)W * H; i++) { uint32_t c = px[i]; px[i] = (c & 0xFF00FF00) | ((c & 0xFF) << 16) | ((c >> 16) & 0xFF); }
}

static uint32_t find_mem(VkPhysicalDeviceMemoryProperties *mp, uint32_t bits, VkMemoryPropertyFlags f)
{
	for (uint32_t i = 0; i < mp->memoryTypeCount; i++)
		if ((bits & (1u << i)) && (mp->memoryTypes[i].propertyFlags & f) == f) return i;
	fprintf(stderr, "no memtype\n"); exit(1);
}

// ---- gaze markers ----
// The shared memory file of `omarchyvr eyed` (linux/crystal-super/omarchyvr_eye_gaze.h): seq_end, body, seq_begin.
typedef struct { int ok; uint32_t flags, state; uint64_t sample; int64_t t_ns; float dir[3][3]; } ShmGaze;
static int shm_fd = -1;
static ShmGaze read_shm(void)
{
	ShmGaze g = {0};
	if (shm_fd < 0) {
		const char *p = getenv("OMARCHYVR_EYE_GAZE_SHM");
		shm_fd = open(p && *p ? p : "/dev/shm/omarchyvr-eye-gaze", O_RDONLY | O_CLOEXEC);
		if (shm_fd < 0) return g;
	}
	for (int a = 0; a < 8; a++) {
		uint8_t b[136];
		if (pread(shm_fd, b + 128, 8, 128) != 8 || pread(shm_fd, b + 8, 120, 8) != 120 || pread(shm_fd, b, 8, 0) != 8) {
			close(shm_fd); shm_fd = -1; return g;
		}
		uint64_t s0, s1; uint32_t magic; memcpy(&s0, b, 8); memcpy(&s1, b + 128, 8); memcpy(&magic, b + 8, 4);
		if (s0 != s1 || !s0 || (s0 & 1)) continue;
		if (magic != 0x4745564fu) return g;
		memcpy(&g.t_ns, b + 16, 8); memcpy(&g.flags, b + 32, 4); memcpy(&g.sample, b + 112, 8); memcpy(&g.state, b + 120, 4);
		for (int i = 0; i < 3; i++) memcpy(g.dir[i], b + 52 + 24 * i, 12);
		g.ok = 1;
		return g;
	}
	return g;
}
static XrVector3f rot_inv(XrQuaternionf q, XrVector3f v) // v rotated by the inverse of q
{
	q.x = -q.x; q.y = -q.y; q.z = -q.z;
	float cx = q.y * v.z - q.z * v.y, cy = q.z * v.x - q.x * v.z, cz = q.x * v.y - q.y * v.x;
	float dx = q.y * cz - q.z * cy, dy = q.z * cx - q.x * cz, dz = q.x * cy - q.y * cx;
	return (XrVector3f){v.x + 2 * (q.w * cx + dx), v.y + 2 * (q.w * cy + dy), v.z + 2 * (q.w * cz + dz)};
}
static XrVector3f rot(XrQuaternionf q, XrVector3f v) { q.x = -q.x; q.y = -q.y; q.z = -q.z; return rot_inv(q, v); }
// A direction in the eye's own frame -> pixel; 0 when behind.
static int to_px(XrFovf fov, int W, int H, XrVector3f d, float *px, float *py)
{
	if (!(d.z < -1e-4f)) return 0;
	float tx = d.x / -d.z, ty = d.y / -d.z;
	float tl = tanf(-fov.angleLeft), tr = tanf(fov.angleRight), tu = tanf(fov.angleUp), td = tanf(-fov.angleDown);
	*px = (tx + tl) / (tl + tr) * W; *py = (tu - ty) / (tu + td) * H;
	return 1;
}
typedef struct { int x0, y0, x1, y1; } Rect;
static Rect dirty[2][4]; static int ndirty[2];
static void restore(uint32_t *px, const uint32_t *base, int W, int e)
{
	for (int i = 0; i < ndirty[e]; i++)
		for (int y = dirty[e][i].y0; y < dirty[e][i].y1; y++)
			memcpy(px + (size_t)y * W + dirty[e][i].x0, base + (size_t)y * W + dirty[e][i].x0,
			       (size_t)(dirty[e][i].x1 - dirty[e][i].x0) * 4);
	ndirty[e] = 0;
}
// A filled disc (ring = 0) or a ring of the given radius in pixels, with a dark outline.
static void marker(uint32_t *px, int W, int H, int e, float cx, float cy, float r, uint32_t col, int ring)
{
	int R = (int)ceilf(r + 3);
	Rect rc = {(int)cx - R, (int)cy - R, (int)cx + R + 1, (int)cy + R + 1};
	if (rc.x0 < 0) rc.x0 = 0; if (rc.y0 < 0) rc.y0 = 0; if (rc.x1 > W) rc.x1 = W; if (rc.y1 > H) rc.y1 = H;
	if (rc.x0 >= rc.x1 || rc.y0 >= rc.y1 || ndirty[e] >= 4) return;
	dirty[e][ndirty[e]++] = rc;
	float w = r * 0.18f + 1.5f;
	for (int y = rc.y0; y < rc.y1; y++)
		for (int x = rc.x0; x < rc.x1; x++) {
			float d = sqrtf((x + 0.5f - cx) * (x + 0.5f - cx) + (y + 0.5f - cy) * (y + 0.5f - cy));
			uint32_t *p = px + (size_t)y * W + x;
			float edge = ring ? fabsf(d - r) : d - r; // < 0 inside the mark
			*p = mix(*p, 0xFF000000, cover(edge - w, 1.5f, 1.0f)); // outline
			*p = mix(*p, col, cover(edge, w, 1.0f));
		}
}
static uint32_t swap_rb(uint32_t c) { return (c & 0xFF00FF00) | ((c & 0xFF) << 16) | ((c >> 16) & 0xFF); }

int main(void)
{
	const char *secs = getenv("XR_GRID_SECONDS");
	double run_s = secs ? atof(secs) : 60;
	const char *ds = getenv("XR_GRID_DIST"); float dist = ds ? (float)atof(ds) : 2.0f;

	const char *gz = getenv("XR_GRID_GAZE"); int want_gaze = !(gz && *gz == '0');
	const char *ext[2] = {XR_KHR_VULKAN_ENABLE2_EXTENSION_NAME, XR_EXT_EYE_GAZE_INTERACTION_EXTENSION_NAME};
	int xr_gaze = 0;
	if (want_gaze) {
		uint32_t ne = 0; xrEnumerateInstanceExtensionProperties(NULL, 0, &ne, NULL);
		XrExtensionProperties *ep = calloc(ne ? ne : 1, sizeof *ep);
		for (uint32_t i = 0; i < ne; i++) ep[i].type = XR_TYPE_EXTENSION_PROPERTIES;
		xrEnumerateInstanceExtensionProperties(NULL, ne, &ne, ep);
		for (uint32_t i = 0; i < ne; i++) if (!strcmp(ep[i].extensionName, XR_EXT_EYE_GAZE_INTERACTION_EXTENSION_NAME)) xr_gaze = 1;
		free(ep);
		printf("runtime eye gaze extension: %s\n", xr_gaze ? "yes" : "no");
	}
	XrInstanceCreateInfo ici = {XR_TYPE_INSTANCE_CREATE_INFO};
	strcpy(ici.applicationInfo.applicationName, "xr_grid");
	ici.applicationInfo.applicationVersion = 1; ici.applicationInfo.apiVersion = XR_MAKE_VERSION(1, 0, 0); // SteamVR takes 1.0 only
	ici.enabledExtensionCount = xr_gaze ? 2 : 1; ici.enabledExtensionNames = ext;
	XrInstance inst; XC(xrCreateInstance(&ici, &inst));
	XrSystemGetInfo sgi = {XR_TYPE_SYSTEM_GET_INFO, 0, XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY};
	XrSystemId sys; XC(xrGetSystem(inst, &sgi, &sys));
	if (xr_gaze) {
		XrSystemEyeGazeInteractionPropertiesEXT eg = {XR_TYPE_SYSTEM_EYE_GAZE_INTERACTION_PROPERTIES_EXT};
		XrSystemProperties sp = {XR_TYPE_SYSTEM_PROPERTIES, &eg};
		XC(xrGetSystemProperties(inst, sys, &sp));
		printf("system %s: eye gaze interaction %s\n", sp.systemName, eg.supportsEyeGazeInteraction ? "supported" : "NOT supported");
		xr_gaze = eg.supportsEyeGazeInteraction != 0;
	}
	XrActionSet aset = XR_NULL_HANDLE; XrAction gaze_act = XR_NULL_HANDLE;
	if (xr_gaze) {
		XrActionSetCreateInfo asi = {XR_TYPE_ACTION_SET_CREATE_INFO}; strcpy(asi.actionSetName, "grid"); strcpy(asi.localizedActionSetName, "Grid");
		XC(xrCreateActionSet(inst, &asi, &aset));
		XrActionCreateInfo aci = {XR_TYPE_ACTION_CREATE_INFO}; strcpy(aci.actionName, "gaze"); strcpy(aci.localizedActionName, "Gaze");
		aci.actionType = XR_ACTION_TYPE_POSE_INPUT; XC(xrCreateAction(aset, &aci, &gaze_act));
		XrActionSuggestedBinding sb = {gaze_act}; XC(xrStringToPath(inst, "/user/eyes_ext/input/gaze_ext/pose", &sb.binding));
		XrInteractionProfileSuggestedBinding ips = {XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING};
		XC(xrStringToPath(inst, "/interaction_profiles/ext/eye_gaze_interaction", &ips.interactionProfile));
		ips.countSuggestedBindings = 1; ips.suggestedBindings = &sb; XC(xrSuggestInteractionProfileBindings(inst, &ips));
	}

	PFN_xrGetVulkanGraphicsRequirements2KHR getReq; PFN_xrCreateVulkanInstanceKHR cvi;
	PFN_xrGetVulkanGraphicsDevice2KHR gpd; PFN_xrCreateVulkanDeviceKHR cvd;
	XC(xrGetInstanceProcAddr(inst, "xrGetVulkanGraphicsRequirements2KHR", (PFN_xrVoidFunction *)&getReq));
	XC(xrGetInstanceProcAddr(inst, "xrCreateVulkanInstanceKHR", (PFN_xrVoidFunction *)&cvi));
	XC(xrGetInstanceProcAddr(inst, "xrGetVulkanGraphicsDevice2KHR", (PFN_xrVoidFunction *)&gpd));
	XC(xrGetInstanceProcAddr(inst, "xrCreateVulkanDeviceKHR", (PFN_xrVoidFunction *)&cvd));
	XrGraphicsRequirementsVulkanKHR req = {XR_TYPE_GRAPHICS_REQUIREMENTS_VULKAN2_KHR};
	XC(getReq(inst, sys, &req));

	void *lib = NULL; (void)lib;
	extern void *dlopen(const char *, int); extern void *dlsym(void *, const char *);
	lib = dlopen("libvulkan.so.1", 2);
	gipa = (PFN_vkGetInstanceProcAddr)dlsym(lib, "vkGetInstanceProcAddr");

	VkApplicationInfo ai = {VK_STRUCTURE_TYPE_APPLICATION_INFO}; ai.apiVersion = VK_API_VERSION_1_1;
	VkInstanceCreateInfo vici = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO}; vici.pApplicationInfo = &ai;
	XrVulkanInstanceCreateInfoKHR xvici = {XR_TYPE_VULKAN_INSTANCE_CREATE_INFO_KHR};
	xvici.systemId = sys; xvici.pfnGetInstanceProcAddr = gipa; xvici.vulkanCreateInfo = &vici;
	VkInstance vki; VkResult vr;
	XC(cvi(inst, &xvici, &vki, &vr)); VC(vr);

	XrVulkanGraphicsDeviceGetInfoKHR gdi = {XR_TYPE_VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR};
	gdi.systemId = sys; gdi.vulkanInstance = vki;
	VkPhysicalDevice pd; XC(gpd(inst, &gdi, &pd));

	VKI(vki, vkGetPhysicalDeviceQueueFamilyProperties); VKI(vki, vkGetPhysicalDeviceMemoryProperties);
	VKI(vki, vkGetDeviceProcAddr);
	uint32_t nq = 0; vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, NULL);
	VkQueueFamilyProperties qp[16]; if (nq > 16) nq = 16; vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, qp);
	uint32_t qf = 0; for (; qf < nq; qf++) if (qp[qf].queueFlags & VK_QUEUE_GRAPHICS_BIT) break;
	float prio = 1; VkDeviceQueueCreateInfo dqi = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
	dqi.queueFamilyIndex = qf; dqi.queueCount = 1; dqi.pQueuePriorities = &prio;
	VkDeviceCreateInfo dci = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO}; dci.queueCreateInfoCount = 1; dci.pQueueCreateInfos = &dqi;
	XrVulkanDeviceCreateInfoKHR xdci = {XR_TYPE_VULKAN_DEVICE_CREATE_INFO_KHR};
	xdci.systemId = sys; xdci.pfnGetInstanceProcAddr = gipa; xdci.vulkanPhysicalDevice = pd; xdci.vulkanCreateInfo = &dci;
	VkDevice dev; XC(cvd(inst, &xdci, &dev, &vr)); VC(vr);

#define VKD(name) PFN_##name name = (PFN_##name)vkGetDeviceProcAddr(dev, #name)
	VKD(vkGetDeviceQueue); VKD(vkCreateCommandPool); VKD(vkAllocateCommandBuffers); VKD(vkBeginCommandBuffer);
	VKD(vkEndCommandBuffer); VKD(vkQueueSubmit); VKD(vkQueueWaitIdle); VKD(vkCreateBuffer);
	VKD(vkGetBufferMemoryRequirements); VKD(vkAllocateMemory); VKD(vkBindBufferMemory); VKD(vkMapMemory);
	VKD(vkCmdCopyBufferToImage); VKD(vkCmdPipelineBarrier); VKD(vkResetCommandBuffer);
	VkQueue q; vkGetDeviceQueue(dev, qf, 0, &q);
	VkCommandPoolCreateInfo cpi = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
	cpi.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT; cpi.queueFamilyIndex = qf;
	VkCommandPool pool; VC(vkCreateCommandPool(dev, &cpi, NULL, &pool));
	VkCommandBufferAllocateInfo cbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
	cbi.commandPool = pool; cbi.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; cbi.commandBufferCount = 1;
	VkCommandBuffer cb; VC(vkAllocateCommandBuffers(dev, &cbi, &cb));
	VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(pd, &mp);

	XrGraphicsBindingVulkan2KHR gb = {XR_TYPE_GRAPHICS_BINDING_VULKAN2_KHR};
	gb.instance = vki; gb.physicalDevice = pd; gb.device = dev; gb.queueFamilyIndex = qf; gb.queueIndex = 0;
	XrSessionCreateInfo sci = {XR_TYPE_SESSION_CREATE_INFO, &gb, 0, sys};
	XrSession sess; XC(xrCreateSession(inst, &sci, &sess));

	XrReferenceSpaceCreateInfo rsi = {XR_TYPE_REFERENCE_SPACE_CREATE_INFO};
	rsi.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_VIEW; rsi.poseInReferenceSpace.orientation.w = 1;
	XrSpace space; XC(xrCreateReferenceSpace(sess, &rsi, &space));
	XrSpace gaze_space = XR_NULL_HANDLE;
	if (xr_gaze) {
		XrActionSpaceCreateInfo asci = {XR_TYPE_ACTION_SPACE_CREATE_INFO}; asci.action = gaze_act; asci.poseInActionSpace.orientation.w = 1;
		XC(xrCreateActionSpace(sess, &asci, &gaze_space));
		XrSessionActionSetsAttachInfo att = {XR_TYPE_SESSION_ACTION_SETS_ATTACH_INFO}; att.countActionSets = 1; att.actionSets = &aset;
		XC(xrAttachSessionActionSets(sess, &att));
	}

	uint32_t nv = 2; XrViewConfigurationView vcv[2] = {{XR_TYPE_VIEW_CONFIGURATION_VIEW}, {XR_TYPE_VIEW_CONFIGURATION_VIEW}};
	XC(xrEnumerateViewConfigurationViews(inst, sys, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 2, &nv, vcv));
	printf("view config: %u views, recommended %ux%u\n", nv, vcv[0].recommendedImageRectWidth, vcv[0].recommendedImageRectHeight);

	uint32_t nf = 0; XC(xrEnumerateSwapchainFormats(sess, 0, &nf, NULL));
	int64_t fmts[64]; if (nf > 64) nf = 64; XC(xrEnumerateSwapchainFormats(sess, nf, &nf, fmts));
	int64_t fmt = 0; int bgra = 0;
	for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_R8G8B8A8_UNORM) { fmt = fmts[i]; bgra = 0; break; }
	if (!fmt) for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_B8G8R8A8_UNORM) { fmt = fmts[i]; bgra = 1; break; }
	// SteamVR offers sRGB only: the bytes are display values already, so they are copied unchanged either way.
	if (!fmt) for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_R8G8B8A8_SRGB) { fmt = fmts[i]; bgra = 0; break; }
	if (!fmt) for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_B8G8R8A8_SRGB) { fmt = fmts[i]; bgra = 1; break; }
	if (!fmt) { fprintf(stderr, "no 8-bit RGBA/BGRA swapchain format\n"); return 1; }

	XrSwapchain sc[2]; VkImage *imgs[2]; uint32_t nimg[2]; uint32_t W = vcv[0].recommendedImageRectWidth, H = vcv[0].recommendedImageRectHeight;
	VkBuffer buf[2]; void *maps[2]; int drawn = 0; uint32_t *base[2];
	for (int e = 0; e < 2; e++) {
		XrSwapchainCreateInfo sc_i = {XR_TYPE_SWAPCHAIN_CREATE_INFO};
		sc_i.usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT | XR_SWAPCHAIN_USAGE_TRANSFER_DST_BIT | XR_SWAPCHAIN_USAGE_SAMPLED_BIT;
		sc_i.format = fmt; sc_i.sampleCount = 1; sc_i.width = W; sc_i.height = H; sc_i.faceCount = 1; sc_i.arraySize = 1; sc_i.mipCount = 1;
		XC(xrCreateSwapchain(sess, &sc_i, &sc[e]));
		XC(xrEnumerateSwapchainImages(sc[e], 0, &nimg[e], NULL));
		XrSwapchainImageVulkan2KHR *xi = calloc(nimg[e], sizeof *xi);
		for (uint32_t i = 0; i < nimg[e]; i++) xi[i].type = XR_TYPE_SWAPCHAIN_IMAGE_VULKAN2_KHR;
		XC(xrEnumerateSwapchainImages(sc[e], nimg[e], &nimg[e], (XrSwapchainImageBaseHeader *)xi));
		imgs[e] = malloc(nimg[e] * sizeof(VkImage)); for (uint32_t i = 0; i < nimg[e]; i++) imgs[e][i] = xi[i].image;

		VkBufferCreateInfo bi = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO}; bi.size = (VkDeviceSize)W * H * 4;
		bi.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT; bi.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
		VC(vkCreateBuffer(dev, &bi, NULL, &buf[e]));
		VkMemoryRequirements mr; vkGetBufferMemoryRequirements(dev, buf[e], &mr);
		VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO}; mai.allocationSize = mr.size;
		mai.memoryTypeIndex = find_mem(&mp, mr.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
		VkDeviceMemory dm; VC(vkAllocateMemory(dev, &mai, NULL, &dm)); VC(vkBindBufferMemory(dev, buf[e], dm, 0));
		void *map; VC(vkMapMemory(dev, dm, 0, VK_WHOLE_SIZE, 0, &map));
		maps[e] = map;
		base[e] = malloc((size_t)W * H * 4);
	}
	double last_print = 0;
	printf("swapchains %ux%u fmt %ld, %u images\n", W, H, (long)fmt, nimg[0]);

	XrSessionState state = XR_SESSION_STATE_UNKNOWN; int running = 0, frames = 0, quit = 0;
	struct timespec t0; clock_gettime(CLOCK_MONOTONIC, &t0); double start = -1;
	while (!quit) {
		XrEventDataBuffer ev = {XR_TYPE_EVENT_DATA_BUFFER};
		while (xrPollEvent(inst, &ev) == XR_SUCCESS) {
			if (ev.type == XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED) {
				state = ((XrEventDataSessionStateChanged *)&ev)->state; printf("session state %d\n", state);
				if (state == XR_SESSION_STATE_READY) {
					XrSessionBeginInfo bi = {XR_TYPE_SESSION_BEGIN_INFO, 0, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO};
					XC(xrBeginSession(sess, &bi)); running = 1;
				} else if (state == XR_SESSION_STATE_STOPPING) { xrEndSession(sess); running = 0; quit = 1; }
				else if (state == XR_SESSION_STATE_EXITING || state == XR_SESSION_STATE_LOSS_PENDING) quit = 1;
			}
			ev.type = XR_TYPE_EVENT_DATA_BUFFER;
		}
		if (!running) { struct timespec ts = {0, 5000000}; nanosleep(&ts, NULL); continue; }
		struct timespec tn; clock_gettime(CLOCK_MONOTONIC, &tn);
		double now = tn.tv_sec + tn.tv_nsec * 1e-9; if (start < 0) start = now;
		if (now - start > run_s) { xrRequestExitSession(sess); start = 1e18; }

		XrFrameState fs = {XR_TYPE_FRAME_STATE}; XC(xrWaitFrame(sess, NULL, &fs));
		XC(xrBeginFrame(sess, NULL));
		XrCompositionLayerProjectionView pv[2] = {{XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW}, {XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW}};
		XrCompositionLayerProjection layer = {XR_TYPE_COMPOSITION_LAYER_PROJECTION}; layer.space = space; layer.viewCount = 2; layer.views = pv;
		const XrCompositionLayerBaseHeader *layers[1] = {(XrCompositionLayerBaseHeader *)&layer}; uint32_t nl = 0;
		if (fs.shouldRender) {
			XrView views[2] = {{XR_TYPE_VIEW}, {XR_TYPE_VIEW}};
			XrViewLocateInfo vli = {XR_TYPE_VIEW_LOCATE_INFO, 0, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, fs.predictedDisplayTime, space};
			XrViewState vs = {XR_TYPE_VIEW_STATE}; uint32_t cnt;
			XC(xrLocateViews(sess, &vli, &vs, 2, &cnt, views));
			if (!drawn) {
				for (int e = 0; e < 2; e++) { draw_grid(maps[e], W, H, e, bgra, views[e].fov, dist); memcpy(base[e], maps[e], (size_t)W * H * 4); }
				drawn = 1;
			}
			if (want_gaze) {
				// the gaze of eyed (head frame) and of the runtime (pose in VIEW space), into each eye's pixels
				ShmGaze sg = read_shm();
				int sg_live = sg.ok && sg.state == 2 && (sg.flags & 7);
				XrSpaceLocation gl = {XR_TYPE_SPACE_LOCATION}; int xr_live = 0;
				if (xr_gaze) {
					XrActiveActionSet as = {aset, XR_NULL_PATH};
					XrActionsSyncInfo syi = {XR_TYPE_ACTIONS_SYNC_INFO}; syi.countActiveActionSets = 1; syi.activeActionSets = &as;
					xrSyncActions(sess, &syi);
					XrActionStateGetInfo gi = {XR_TYPE_ACTION_STATE_GET_INFO}; gi.action = gaze_act;
					XrActionStatePose ps = {XR_TYPE_ACTION_STATE_POSE};
					if (XR_SUCCEEDED(xrGetActionStatePose(sess, &gi, &ps)) && ps.isActive &&
					    XR_SUCCEEDED(xrLocateSpace(gaze_space, space, fs.predictedDisplayTime, &gl)) &&
					    (gl.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT)) xr_live = 1;
				}
				const uint32_t cyan = bgra ? swap_rb(0xFFFFFF00) : 0xFFFFFF00, magenta = bgra ? swap_rb(0xFFFF00FF) : 0xFFFF00FF;
				float pix = (tanf(-views[0].fov.angleLeft) + tanf(views[0].fov.angleRight)) / W;
				float r_dot = tanf(0.6f * 3.14159265f / 180.f) / pix, r_ring = tanf(1.2f * 3.14159265f / 180.f) / pix;
				XrVector3f xr_dir = rot(gl.pose.orientation, (XrVector3f){0, 0, -1});
				for (int e = 0; e < 2; e++) {
					restore(maps[e], base[e], W, e);
					float x, y;
					if (sg_live) {
						int k = (sg.flags & (e == 0 ? 1 : 2)) ? e : 2; // this eye's ray, else the combined one
						XrVector3f d = {sg.dir[k][0], sg.dir[k][1], sg.dir[k][2]};
						if (to_px(views[e].fov, W, H, rot_inv(views[e].pose.orientation, d), &x, &y))
							marker(maps[e], W, H, e, x, y, r_dot, cyan, 0);
					}
					if (xr_live) {
						XrVector3f p = {gl.pose.position.x + xr_dir.x * dist - views[e].pose.position.x,
						                gl.pose.position.y + xr_dir.y * dist - views[e].pose.position.y,
						                gl.pose.position.z + xr_dir.z * dist - views[e].pose.position.z};
						if (to_px(views[e].fov, W, H, rot_inv(views[e].pose.orientation, p), &x, &y))
							marker(maps[e], W, H, e, x, y, r_ring, magenta, 1);
					}
				}
				if (now - last_print >= 0.5) {
					last_print = now;
					const float k = 180.f / 3.14159265f;
					if (sg_live)
						printf("eyed  L %+6.1f %+6.1f  R %+6.1f %+6.1f deg (right, up)  flags %u", atan2f(sg.dir[0][0], -sg.dir[0][2]) * k,
						       atan2f(sg.dir[0][1], -sg.dir[0][2]) * k, atan2f(sg.dir[1][0], -sg.dir[1][2]) * k, atan2f(sg.dir[1][1], -sg.dir[1][2]) * k, sg.flags);
					else printf("eyed  no gaze%s", sg.ok ? "" : " (no /dev/shm/omarchyvr-eye-gaze)");
					if (xr_live) printf(" | openxr %+6.1f %+6.1f deg\n", atan2f(xr_dir.x, -xr_dir.z) * k, atan2f(xr_dir.y, -xr_dir.z) * k);
					else printf(" | openxr %s\n", xr_gaze ? "no gaze" : "not offered");
					fflush(stdout);
				}
			}
			for (int e = 0; e < 2; e++) {
				uint32_t idx; XC(xrAcquireSwapchainImage(sc[e], NULL, &idx));
				XrSwapchainImageWaitInfo wi = {XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO, 0, XR_INFINITE_DURATION}; XC(xrWaitSwapchainImage(sc[e], &wi));
				VkCommandBufferBeginInfo bgi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO}; bgi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
				vkResetCommandBuffer(cb, 0); VC(vkBeginCommandBuffer(cb, &bgi));
				VkImageMemoryBarrier b = {VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
				b.srcQueueFamilyIndex = b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; b.image = imgs[e][idx];
				b.subresourceRange = (VkImageSubresourceRange){VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
				b.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED; b.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; b.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
				vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &b);
				VkBufferImageCopy rg = {0}; rg.imageSubresource = (VkImageSubresourceLayers){VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
				rg.imageExtent = (VkExtent3D){W, H, 1};
				vkCmdCopyBufferToImage(cb, buf[e], imgs[e][idx], VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &rg);
				b.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; b.newLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
				b.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; b.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
				vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1, &b);
				VC(vkEndCommandBuffer(cb));
				VkSubmitInfo si = {VK_STRUCTURE_TYPE_SUBMIT_INFO}; si.commandBufferCount = 1; si.pCommandBuffers = &cb;
				VC(vkQueueSubmit(q, 1, &si, VK_NULL_HANDLE)); VC(vkQueueWaitIdle(q));
				XC(xrReleaseSwapchainImage(sc[e], NULL));
				pv[e].pose = views[e].pose; pv[e].fov = views[e].fov;
				pv[e].subImage.swapchain = sc[e]; pv[e].subImage.imageRect = (XrRect2Di){{0, 0}, {(int)W, (int)H}};
			}
			nl = 1;
		}
		XrFrameEndInfo fe = {XR_TYPE_FRAME_END_INFO, 0, fs.predictedDisplayTime, XR_ENVIRONMENT_BLEND_MODE_OPAQUE, nl, layers};
		XC(xrEndFrame(sess, &fe)); frames++;
		if (frames % 72 == 0) printf("frames %d\n", frames);
	}
	printf("done, %d frames\n", frames);
	return 0;
}
