/* Headless OpenXR pose printer (XR_MND_headless). Usage: posecli [seconds]
 * Prints VIEW-in-LOCAL pose 10x/s as quaternion + yaw/pitch/roll (deg). */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#define XR_USE_TIMESPEC
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>

#define CK(x) do { XrResult r_ = (x); if (XR_FAILED(r_)) { fprintf(stderr, "%s failed: %d\n", #x, r_); exit(1);} } while (0)

int main(int argc, char **argv)
{
	double secs = argc > 1 ? atof(argv[1]) : 10;
	/* POSECLI_OVERLAY=1: also an XR_EXTX_overlay session, so a main app (passthrough) keeps the display. */
	int overlay = getenv("POSECLI_OVERLAY") && atoi(getenv("POSECLI_OVERLAY"));
	const char *exts[] = {"XR_MND_headless", "XR_KHR_convert_timespec_time", "XR_EXTX_overlay"};
	XrInstanceCreateInfo ici = {XR_TYPE_INSTANCE_CREATE_INFO};
	strcpy(ici.applicationInfo.applicationName, "posecli");
	ici.applicationInfo.apiVersion = XR_MAKE_VERSION(1, 0, 0);
	ici.enabledExtensionCount = overlay ? 3 : 2;
	ici.enabledExtensionNames = exts;
	XrInstance inst;
	CK(xrCreateInstance(&ici, &inst));
	XrSystemGetInfo sgi = {XR_TYPE_SYSTEM_GET_INFO, NULL, XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY};
	XrSystemId sys;
	CK(xrGetSystem(inst, &sgi, &sys));
	XrSessionCreateInfo sci = {XR_TYPE_SESSION_CREATE_INFO};
	sci.systemId = sys;
	XrSessionCreateInfoOverlayEXTX ov = {XR_TYPE_SESSION_CREATE_INFO_OVERLAY_EXTX, NULL, 0, 1};
	if (overlay)
		sci.next = &ov;
	XrSession ses;
	CK(xrCreateSession(inst, &sci, &ses));
	XrReferenceSpaceCreateInfo rci = {XR_TYPE_REFERENCE_SPACE_CREATE_INFO};
	rci.poseInReferenceSpace.orientation.w = 1;
	XrSpace local, view;
	rci.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL;
	CK(xrCreateReferenceSpace(ses, &rci, &local));
	rci.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_VIEW;
	CK(xrCreateReferenceSpace(ses, &rci, &view));

	int running = 0;
	struct timespec t0, now, sl = {0, 100 * 1000000L};
	clock_gettime(CLOCK_MONOTONIC, &t0);
	for (;;) {
		XrEventDataBuffer ev = {XR_TYPE_EVENT_DATA_BUFFER};
		while (xrPollEvent(inst, &ev) == XR_SUCCESS) {
			if (ev.type == XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED) {
				XrEventDataSessionStateChanged *s = (void *)&ev;
				printf("state -> %d\n", s->state);
				if (s->state == XR_SESSION_STATE_READY && !running) {
					XrSessionBeginInfo bi = {XR_TYPE_SESSION_BEGIN_INFO};
					bi.primaryViewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
					CK(xrBeginSession(ses, &bi));
					running = 1;
				}
			}
			ev = (XrEventDataBuffer){XR_TYPE_EVENT_DATA_BUFFER};
		}
		clock_gettime(CLOCK_MONOTONIC, &now);
		double el = (now.tv_sec - t0.tv_sec) + (now.tv_nsec - t0.tv_nsec) / 1e9;
		if (el > secs) break;
		if (running) {
			/* headless: no xrWaitFrame; use current time via monotonic conversion */
			PFN_xrConvertTimespecTimeToTimeKHR conv;
			xrGetInstanceProcAddr(inst, "xrConvertTimespecTimeToTimeKHR", (PFN_xrVoidFunction *)&conv);
			XrTime t = 0;
			if (conv) conv(inst, &now, &t);
			XrSpaceLocation loc = {XR_TYPE_SPACE_LOCATION};
			if (XR_SUCCEEDED(xrLocateSpace(view, local, t, &loc))) {
				XrQuaternionf q = loc.pose.orientation;
				double yaw = atan2(2 * (q.w * q.y + q.x * q.z), 1 - 2 * (q.y * q.y + q.x * q.x));
				double pitch = asin(fmax(-1, fmin(1, 2 * (q.w * q.x - q.y * q.z))));
				double roll = atan2(2 * (q.w * q.z + q.x * q.y), 1 - 2 * (q.z * q.z + q.x * q.x));
				XrVector3f p = loc.pose.position;
				printf("t=%5.1f flags=0x%x q=(%.4f %.4f %.4f %.4f) yaw=%.2f pitch=%.2f roll=%.2f p=(%+.3f %+.3f %+.3f)%s\n",
				       el, (unsigned)loc.locationFlags, q.x, q.y, q.z, q.w, yaw * 57.2958, pitch * 57.2958,
				       roll * 57.2958, p.x, p.y, p.z,
				       (loc.locationFlags & XR_SPACE_LOCATION_POSITION_TRACKED_BIT) ? " 6dof" : "");
			}
		}
		nanosleep(&sl, NULL);
	}
	if (running) xrEndSession(ses);
	xrDestroyInstance(inst);
	return 0;
}
