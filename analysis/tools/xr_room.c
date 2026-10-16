// World-locked "sky sphere" room for head-rotation checks: a fragment shader draws, per eye, a lat/lon grid,
// horizon, 6 coloured cardinal markers (front green F, back red, left blue, right yellow, up white, floor checker).
// Build: gcc -O2 -o xr_room xr_room.c -lopenxr_loader -lvulkan -lm -ldl
// Shaders: glslangValidator -V --vn xr_room_vert_spv -o xr_room_vert.h xr_room.vert (same for .frag)
// Run:   XR_ROOM_SECONDS=20 ./xr_room
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
#include "xr_room_vert.h"
#include "xr_room_frag.h"

#define XC(x) do { XrResult r_ = (x); if (XR_FAILED(r_)) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)
#define VC(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s:%d %s -> %d\n", __FILE__, __LINE__, #x, r_); exit(1);} } while (0)

static PFN_vkGetInstanceProcAddr gipa;
#define VKI(inst, name) PFN_##name name = (PFN_##name)gipa(inst, #name)

static uint32_t find_mem(VkPhysicalDeviceMemoryProperties *mp, uint32_t bits, VkMemoryPropertyFlags f)
{
	for (uint32_t i = 0; i < mp->memoryTypeCount; i++)
		if ((bits & (1u << i)) && (mp->memoryTypes[i].propertyFlags & f) == f) return i;
	fprintf(stderr, "no memtype\n"); exit(1);
}


typedef struct { float c0[4], c1[4], c2[4], tan_lrud[4]; } PC;

int main(void)
{
	const char *secs = getenv("XR_ROOM_SECONDS");
	double run_s = secs ? atof(secs) : 60;
	const char *ext[] = {XR_KHR_VULKAN_ENABLE2_EXTENSION_NAME};
	XrInstanceCreateInfo ici = {XR_TYPE_INSTANCE_CREATE_INFO};
	strcpy(ici.applicationInfo.applicationName, "xr_room");
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

	extern void *dlopen(const char *, int); extern void *dlsym(void *, const char *);
	void *lib = dlopen("libvulkan.so.1", 2);
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

	VKI(vki, vkGetPhysicalDeviceQueueFamilyProperties); VKI(vki, vkGetDeviceProcAddr);
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
	VKD(vkEndCommandBuffer); VKD(vkQueueSubmit); VKD(vkQueueWaitIdle); VKD(vkResetCommandBuffer);
	VKD(vkCreateRenderPass); VKD(vkCreateImageView); VKD(vkCreateFramebuffer); VKD(vkCreateShaderModule);
	VKD(vkCreatePipelineLayout); VKD(vkCreateGraphicsPipelines); VKD(vkCmdBeginRenderPass); VKD(vkCmdEndRenderPass);
	VKD(vkCmdBindPipeline); VKD(vkCmdPushConstants); VKD(vkCmdDraw); VKD(vkCmdSetViewport); VKD(vkCmdSetScissor);
	VkQueue q; vkGetDeviceQueue(dev, qf, 0, &q);
	VkCommandPoolCreateInfo cpi = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
	cpi.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT; cpi.queueFamilyIndex = qf;
	VkCommandPool pool; VC(vkCreateCommandPool(dev, &cpi, NULL, &pool));
	VkCommandBufferAllocateInfo cbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
	cbi.commandPool = pool; cbi.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; cbi.commandBufferCount = 1;
	VkCommandBuffer cb; VC(vkAllocateCommandBuffers(dev, &cbi, &cb));

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
	int64_t fmt = 0;
	for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_R8G8B8A8_UNORM) { fmt = fmts[i]; break; }
	if (!fmt) for (uint32_t i = 0; i < nf; i++) if (fmts[i] == VK_FORMAT_B8G8R8A8_UNORM) { fmt = fmts[i]; break; }
	if (!fmt) { fprintf(stderr, "no 8-bit UNORM swapchain format\n"); return 1; }

	// render pass + pipeline
	VkAttachmentDescription ad = {0}; ad.format = (VkFormat)fmt; ad.samples = VK_SAMPLE_COUNT_1_BIT;
	ad.loadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE; ad.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
	ad.stencilLoadOp = ad.stencilStoreOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
	ad.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED; ad.finalLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
	VkAttachmentReference ar = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
	VkSubpassDescription sp = {0}; sp.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS; sp.colorAttachmentCount = 1; sp.pColorAttachments = &ar;
	VkRenderPassCreateInfo rpi = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO}; rpi.attachmentCount = 1; rpi.pAttachments = &ad;
	rpi.subpassCount = 1; rpi.pSubpasses = &sp;
	VkRenderPass rp; VC(vkCreateRenderPass(dev, &rpi, NULL, &rp));

	VkShaderModuleCreateInfo smi = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
	VkShaderModule vs_m, fs_m;
	smi.codeSize = sizeof xr_room_vert_spv; smi.pCode = xr_room_vert_spv; VC(vkCreateShaderModule(dev, &smi, NULL, &vs_m));
	smi.codeSize = sizeof xr_room_frag_spv; smi.pCode = xr_room_frag_spv; VC(vkCreateShaderModule(dev, &smi, NULL, &fs_m));
	VkPushConstantRange pcr = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PC)};
	VkPipelineLayoutCreateInfo pli = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO}; pli.pushConstantRangeCount = 1; pli.pPushConstantRanges = &pcr;
	VkPipelineLayout pl; VC(vkCreatePipelineLayout(dev, &pli, NULL, &pl));
	VkPipelineShaderStageCreateInfo st[2] = {{VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, 0, 0, VK_SHADER_STAGE_VERTEX_BIT, vs_m, "main"},
		{VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, 0, 0, VK_SHADER_STAGE_FRAGMENT_BIT, fs_m, "main"}};
	VkPipelineVertexInputStateCreateInfo vis = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
	VkPipelineInputAssemblyStateCreateInfo ias = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO}; ias.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
	VkPipelineViewportStateCreateInfo vps = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO}; vps.viewportCount = 1; vps.scissorCount = 1;
	VkPipelineRasterizationStateCreateInfo rs = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
	rs.polygonMode = VK_POLYGON_MODE_FILL; rs.cullMode = VK_CULL_MODE_NONE; rs.lineWidth = 1;
	VkPipelineMultisampleStateCreateInfo ms = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO}; ms.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
	VkPipelineColorBlendAttachmentState cba = {0}; cba.colorWriteMask = 0xF;
	VkPipelineColorBlendStateCreateInfo cbs = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO}; cbs.attachmentCount = 1; cbs.pAttachments = &cba;
	VkDynamicState dyn[2] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
	VkPipelineDynamicStateCreateInfo dys = {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO}; dys.dynamicStateCount = 2; dys.pDynamicStates = dyn;
	VkGraphicsPipelineCreateInfo gpi = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO}; gpi.stageCount = 2; gpi.pStages = st;
	gpi.pVertexInputState = &vis; gpi.pInputAssemblyState = &ias; gpi.pViewportState = &vps; gpi.pRasterizationState = &rs;
	gpi.pMultisampleState = &ms; gpi.pColorBlendState = &cbs; gpi.pDynamicState = &dys; gpi.layout = pl; gpi.renderPass = rp;
	VkPipeline pipe; VC(vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &gpi, NULL, &pipe));

	XrSwapchain sc[2]; uint32_t nimg[2]; VkFramebuffer *fbs[2]; uint32_t W = vcv[0].recommendedImageRectWidth, H = vcv[0].recommendedImageRectHeight;
	for (int e = 0; e < 2; e++) {
		XrSwapchainCreateInfo sc_i = {XR_TYPE_SWAPCHAIN_CREATE_INFO};
		sc_i.usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT | XR_SWAPCHAIN_USAGE_SAMPLED_BIT;
		sc_i.format = fmt; sc_i.sampleCount = 1; sc_i.width = W; sc_i.height = H; sc_i.faceCount = 1; sc_i.arraySize = 1; sc_i.mipCount = 1;
		XC(xrCreateSwapchain(sess, &sc_i, &sc[e]));
		XC(xrEnumerateSwapchainImages(sc[e], 0, &nimg[e], NULL));
		XrSwapchainImageVulkan2KHR *xi = calloc(nimg[e], sizeof *xi);
		for (uint32_t i = 0; i < nimg[e]; i++) xi[i].type = XR_TYPE_SWAPCHAIN_IMAGE_VULKAN2_KHR;
		XC(xrEnumerateSwapchainImages(sc[e], nimg[e], &nimg[e], (XrSwapchainImageBaseHeader *)xi));
		fbs[e] = malloc(nimg[e] * sizeof(VkFramebuffer));
		for (uint32_t i = 0; i < nimg[e]; i++) {
			VkImageViewCreateInfo vi = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO}; vi.image = xi[i].image;
			vi.viewType = VK_IMAGE_VIEW_TYPE_2D; vi.format = (VkFormat)fmt;
			vi.subresourceRange = (VkImageSubresourceRange){VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
			VkImageView iv; VC(vkCreateImageView(dev, &vi, NULL, &iv));
			VkFramebufferCreateInfo fi = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO}; fi.renderPass = rp; fi.attachmentCount = 1;
			fi.pAttachments = &iv; fi.width = W; fi.height = H; fi.layers = 1;
			VC(vkCreateFramebuffer(dev, &fi, NULL, &fbs[e][i]));
		}
	}
	printf("swapchains %ux%u fmt %ld, %u images\n", W, H, (long)fmt, nimg[0]);

	XrSessionState state = XR_SESSION_STATE_UNKNOWN; int running = 0, frames = 0, quit = 0;
	struct timespec t0; clock_gettime(CLOCK_MONOTONIC, &t0); double start = -1, lastp = 0;
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
		double now = tn.tv_sec + tn.tv_nsec * 1e-9; if (start < 0) { start = now; lastp = now; }
		if (now - start > run_s) { xrRequestExitSession(sess); start = 1e18; }

		XrFrameState fs = {XR_TYPE_FRAME_STATE}; XC(xrWaitFrame(sess, NULL, &fs));
		XC(xrBeginFrame(sess, NULL));
		XrCompositionLayerProjectionView pv[2] = {{XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW}, {XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW}};
		XrCompositionLayerProjection layer = {XR_TYPE_COMPOSITION_LAYER_PROJECTION}; layer.space = space; layer.viewCount = 2; layer.views = pv;
		const XrCompositionLayerBaseHeader *layers[1] = {(XrCompositionLayerBaseHeader *)&layer}; uint32_t nl = 0;
		if (fs.shouldRender) {
			XrView views[2] = {{XR_TYPE_VIEW}, {XR_TYPE_VIEW}};
			XrViewLocateInfo vli = {XR_TYPE_VIEW_LOCATE_INFO, 0, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, fs.predictedDisplayTime, space};
			XrViewState vst = {XR_TYPE_VIEW_STATE}; uint32_t cnt;
			XC(xrLocateViews(sess, &vli, &vst, 2, &cnt, views));
			for (int e = 0; e < 2; e++) {
				uint32_t idx; XC(xrAcquireSwapchainImage(sc[e], NULL, &idx));
				XrSwapchainImageWaitInfo wi = {XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO, 0, XR_INFINITE_DURATION}; XC(xrWaitSwapchainImage(sc[e], &wi));
				// view->world rotation matrix from the pose quaternion (columns)
				float x = views[e].pose.orientation.x, y = views[e].pose.orientation.y, z = views[e].pose.orientation.z, w = views[e].pose.orientation.w;
				float n = sqrtf(x * x + y * y + z * z + w * w); if (n < 1e-6f) { x = y = z = 0; w = 1; } else { x /= n; y /= n; z /= n; w /= n; }
				PC pc = {{1 - 2 * (y * y + z * z), 2 * (x * y + z * w), 2 * (x * z - y * w), 0},
					 {2 * (x * y - z * w), 1 - 2 * (x * x + z * z), 2 * (y * z + x * w), 0},
					 {2 * (x * z + y * w), 2 * (y * z - x * w), 1 - 2 * (x * x + y * y), 0},
					 {tanf(views[e].fov.angleLeft), tanf(views[e].fov.angleRight), tanf(views[e].fov.angleUp), tanf(views[e].fov.angleDown)}};
				VkCommandBufferBeginInfo bgi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO}; bgi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
				vkResetCommandBuffer(cb, 0); VC(vkBeginCommandBuffer(cb, &bgi));
				VkRenderPassBeginInfo rbi = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO}; rbi.renderPass = rp; rbi.framebuffer = fbs[e][idx];
				rbi.renderArea = (VkRect2D){{0, 0}, {W, H}};
				vkCmdBeginRenderPass(cb, &rbi, VK_SUBPASS_CONTENTS_INLINE);
				vkCmdBindPipeline(cb, VK_PIPELINE_BIND_POINT_GRAPHICS, pipe);
				VkViewport vp = {0, 0, (float)W, (float)H, 0, 1}; VkRect2D sc2 = {{0, 0}, {W, H}};
				vkCmdSetViewport(cb, 0, 1, &vp); vkCmdSetScissor(cb, 0, 1, &sc2);
				vkCmdPushConstants(cb, pl, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof pc, &pc);
				vkCmdDraw(cb, 3, 1, 0, 0);
				vkCmdEndRenderPass(cb);
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
		if (now - lastp >= 5) { printf("frames %d (%.1f fps)\n", frames, frames / (now - start)); fflush(stdout); lastp = now; }
	}
	printf("done, %d frames\n", frames);
	return 0;
}
