// Minimal Vulkan OpenXR test client: per-eye colour, white arrow pointing UP, letter L/R, yellow marker
// in the top-left corner of each eye image. CPU renders the bitmap once, copied to the swapchain per frame.
// Build: gcc -O2 -o xr_eyetest_srgb xr_eyetest_srgb.c -lopenxr_loader -lvulkan
// Run:   XR_EYETEST_SECONDS=20 ./xr_eyetest_srgb
#define XR_USE_GRAPHICS_API_VULKAN
#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define XC(x) do { XrResult r_ = (x); if (XR_FAILED(r_)) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)
#define VC(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)

static PFN_vkGetInstanceProcAddr gipa;
#define VKI(inst, name) PFN_##name name = (PFN_##name)gipa(inst, #name)

static void rect(uint32_t *px, int W, int H, int x0, int y0, int x1, int y1, uint32_t c)
{
	if (x0 < 0) x0 = 0; if (y0 < 0) y0 = 0; if (x1 > W) x1 = W; if (y1 > H) y1 = H;
	for (int y = y0; y < y1; y++) for (int x = x0; x < x1; x++) px[(size_t)y * W + x] = c;
}

// colours as 0xAABBGGRR (R8G8B8A8 byte order); swapped for BGRA formats afterwards
static void draw_eye(uint32_t *px, int W, int H, int eye, int bgra)
{
	uint32_t bg = eye == 0 ? 0xFF802000 : 0xFF202080; // L: blue-ish, R: red-ish
	uint32_t white = 0xFFFFFFFF, yellow = 0xFF00FFFF;
	rect(px, W, H, 0, 0, W, H, bg);
	int b = W / 100;
	rect(px, W, H, 0, 0, W, b, white); rect(px, W, H, 0, H - b, W, H, white);
	rect(px, W, H, 0, 0, b, H, white); rect(px, W, H, W - b, 0, W, H, white);
	rect(px, W, H, b * 2, b * 2, b * 2 + W / 12, b * 2 + W / 12, yellow); // top-left marker
	int cx = W / 2, top = H / 8, headh = H / 4, hw = W / 4, sw = W / 16, sh = H / 2 - H / 16;
	for (int y = 0; y < headh; y++) { // triangle head, apex up
		int half = hw * y / headh;
		rect(px, W, H, cx - half, top + y, cx + half, top + y + 1, white);
	}
	rect(px, W, H, cx - sw / 2, top + headh, cx + sw / 2, top + headh + sh, white);
	// letter, bottom centre, ~ H/4 tall
	int lh = H / 4, lw = lh * 3 / 5, t = lh / 6, lx = cx - lw / 2, ly = H - H / 8 - lh;
	uint32_t ink = 0xFF000000;
	rect(px, W, H, lx, ly, lx + t, ly + lh, ink); // vertical stem
	if (eye == 0) {
		rect(px, W, H, lx, ly + lh - t, lx + lw, ly + lh, ink);
	} else {
		rect(px, W, H, lx, ly, lx + lw, ly + t, ink);
		rect(px, W, H, lx, ly + lh / 2 - t / 2, lx + lw, ly + lh / 2 + t / 2, ink);
		rect(px, W, H, lx + lw - t, ly, lx + lw, ly + lh / 2 + t / 2, ink);
		int n = 12; // diagonal leg
		for (int i = 0; i < n; i++) {
			int y0 = ly + lh / 2 + t / 2 + (lh / 2 - t / 2) * i / n, y1 = ly + lh / 2 + t / 2 + (lh / 2 - t / 2) * (i + 1) / n;
			int xs = lx + t + (lw - 2 * t) * i / n;
			rect(px, W, H, xs, y0, xs + t, y1, ink);
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

int main(void)
{
	const char *secs = getenv("XR_EYETEST_SECONDS");
	double run_s = secs ? atof(secs) : 20;

	const char *ext[] = {XR_KHR_VULKAN_ENABLE2_EXTENSION_NAME};
	XrInstanceCreateInfo ici = {XR_TYPE_INSTANCE_CREATE_INFO};
	strcpy(ici.applicationInfo.applicationName, "xr_eyetest_srgb");
	ici.applicationInfo.applicationVersion = 1; ici.applicationInfo.apiVersion = XR_CURRENT_API_VERSION;
	ici.enabledExtensionCount = 1; ici.enabledExtensionNames = ext;
	XrInstance inst; XC(xrCreateInstance(&ici, &inst));
	XrSystemGetInfo sgi = {XR_TYPE_SYSTEM_GET_INFO, 0, XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY};
	XrSystemId sys; XC(xrGetSystem(inst, &sgi, &sys));

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
	rsi.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL; rsi.poseInReferenceSpace.orientation.w = 1;
	XrSpace space; XC(xrCreateReferenceSpace(sess, &rsi, &space));

	uint32_t nv = 2; XrViewConfigurationView vcv[2] = {{XR_TYPE_VIEW_CONFIGURATION_VIEW}, {XR_TYPE_VIEW_CONFIGURATION_VIEW}};
	XC(xrEnumerateViewConfigurationViews(inst, sys, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 2, &nv, vcv));
	printf("view config: %u views, recommended %ux%u\n", nv, vcv[0].recommendedImageRectWidth, vcv[0].recommendedImageRectHeight);

	uint32_t nf = 0; XC(xrEnumerateSwapchainFormats(sess, 0, &nf, NULL));
	int64_t fmts[64]; if (nf > 64) nf = 64; XC(xrEnumerateSwapchainFormats(sess, nf, &nf, fmts));
	int64_t fmt = 0; int bgra = 0;
	for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_R8G8B8A8_SRGB) { fmt = fmts[i]; bgra = 0; break; }
	if (!fmt) for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_B8G8R8A8_SRGB) { fmt = fmts[i]; bgra = 1; break; }
	if (!fmt) { fprintf(stderr, "no 8-bit UNORM swapchain format\n"); return 1; }

	XrSwapchain sc[2]; VkImage *imgs[2]; uint32_t nimg[2]; uint32_t W = vcv[0].recommendedImageRectWidth, H = vcv[0].recommendedImageRectHeight;
	VkBuffer buf[2]; 
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
		draw_eye(map, W, H, e, bgra);
	}
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
