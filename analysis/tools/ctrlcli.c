/* Headless OpenXR controller printer (XR_MND_headless), Oculus Touch profile. Usage: ctrlcli [seconds]
 * Prints trigger/grip/A(X)/B(Y)/stick/stick-click/menu + aim pose for both hands 5x/s.
 * Build: gcc -o ctrlcli analysis/tools/ctrlcli.c -lopenxr_loader -lm */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#define XR_USE_TIMESPEC
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>

#define CK(x) do { XrResult r_ = (x); if (XR_FAILED(r_)) { fprintf(stderr, "%s failed: %d\n", #x, r_); exit(1);} } while (0)

static XrInstance inst;
static XrPath P(const char *s) { XrPath p; CK(xrStringToPath(inst, s, &p)); return p; }

static XrAction mk(XrActionSet set, const char *name, XrActionType t, XrPath *sub)
{
	XrActionCreateInfo ai = {XR_TYPE_ACTION_CREATE_INFO};
	strcpy(ai.actionName, name); strcpy(ai.localizedActionName, name);
	ai.actionType = t; ai.countSubactionPaths = 2; ai.subactionPaths = sub;
	XrAction a; CK(xrCreateAction(set, &ai, &a)); return a;
}

int main(int argc, char **argv)
{
	double secs = argc > 1 ? atof(argv[1]) : 20;
	const char *exts[] = {"XR_MND_headless", "XR_KHR_convert_timespec_time"};
	XrInstanceCreateInfo ici = {XR_TYPE_INSTANCE_CREATE_INFO};
	strcpy(ici.applicationInfo.applicationName, "ctrlcli");
	ici.applicationInfo.apiVersion = XR_MAKE_VERSION(1, 0, 0);
	ici.enabledExtensionCount = 2; ici.enabledExtensionNames = exts;
	CK(xrCreateInstance(&ici, &inst));
	XrSystemGetInfo sgi = {XR_TYPE_SYSTEM_GET_INFO, NULL, XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY};
	XrSystemId sys; CK(xrGetSystem(inst, &sgi, &sys));
	XrSessionCreateInfo sci = {XR_TYPE_SESSION_CREATE_INFO}; sci.systemId = sys;
	XrSession ses; CK(xrCreateSession(inst, &sci, &ses));

	XrPath hand[2] = {P("/user/hand/left"), P("/user/hand/right")};
	XrActionSetCreateInfo si = {XR_TYPE_ACTION_SET_CREATE_INFO};
	strcpy(si.actionSetName, "ctl"); strcpy(si.localizedActionSetName, "ctl");
	XrActionSet set; CK(xrCreateActionSet(inst, &si, &set));
	XrAction trig = mk(set, "trigger", XR_ACTION_TYPE_FLOAT_INPUT, hand);
	XrAction grip = mk(set, "grip", XR_ACTION_TYPE_FLOAT_INPUT, hand);
	XrAction btn1 = mk(set, "a_or_x", XR_ACTION_TYPE_BOOLEAN_INPUT, hand);
	XrAction btn2 = mk(set, "b_or_y", XR_ACTION_TYPE_BOOLEAN_INPUT, hand);
	XrAction stick = mk(set, "stick", XR_ACTION_TYPE_VECTOR2F_INPUT, hand);
	XrAction sclick = mk(set, "stick_click", XR_ACTION_TYPE_BOOLEAN_INPUT, hand);
	XrAction menu = mk(set, "menu", XR_ACTION_TYPE_BOOLEAN_INPUT, hand);
	XrAction aim = mk(set, "aim", XR_ACTION_TYPE_POSE_INPUT, hand);

	XrActionSuggestedBinding b[] = {
	    {trig, P("/user/hand/left/input/trigger/value")},  {trig, P("/user/hand/right/input/trigger/value")},
	    {grip, P("/user/hand/left/input/squeeze/value")},  {grip, P("/user/hand/right/input/squeeze/value")},
	    {btn1, P("/user/hand/left/input/x/click")},        {btn1, P("/user/hand/right/input/a/click")},
	    {btn2, P("/user/hand/left/input/y/click")},        {btn2, P("/user/hand/right/input/b/click")},
	    {stick, P("/user/hand/left/input/thumbstick")},    {stick, P("/user/hand/right/input/thumbstick")},
	    {sclick, P("/user/hand/left/input/thumbstick/click")}, {sclick, P("/user/hand/right/input/thumbstick/click")},
	    {menu, P("/user/hand/left/input/menu/click")},
	    {aim, P("/user/hand/left/input/aim/pose")},        {aim, P("/user/hand/right/input/aim/pose")},
	};
	XrInteractionProfileSuggestedBinding sb = {XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING};
	sb.interactionProfile = P("/interaction_profiles/oculus/touch_controller");
	sb.countSuggestedBindings = sizeof b / sizeof b[0]; sb.suggestedBindings = b;
	CK(xrSuggestInteractionProfileBindings(inst, &sb));

	XrReferenceSpaceCreateInfo rci = {XR_TYPE_REFERENCE_SPACE_CREATE_INFO};
	rci.poseInReferenceSpace.orientation.w = 1; rci.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL;
	XrSpace local; CK(xrCreateReferenceSpace(ses, &rci, &local));
	XrSpace aimsp[2];
	for (int i = 0; i < 2; i++) {
		XrActionSpaceCreateInfo ci = {XR_TYPE_ACTION_SPACE_CREATE_INFO};
		ci.action = aim; ci.subactionPath = hand[i]; ci.poseInActionSpace.orientation.w = 1;
		CK(xrCreateActionSpace(ses, &ci, &aimsp[i]));
	}
	XrSessionActionSetsAttachInfo ai = {XR_TYPE_SESSION_ACTION_SETS_ATTACH_INFO};
	ai.countActionSets = 1; ai.actionSets = &set;
	CK(xrAttachSessionActionSets(ses, &ai));

	PFN_xrConvertTimespecTimeToTimeKHR conv;
	xrGetInstanceProcAddr(inst, "xrConvertTimespecTimeToTimeKHR", (PFN_xrVoidFunction *)&conv);
	int running = 0, focused = 0, printed_profile = 0;
	struct timespec t0, now, sl = {0, 200 * 1000000L};
	clock_gettime(CLOCK_MONOTONIC, &t0);
	printf("press buttons / squeeze triggers on the controllers (wake them with a button press)\n");
	for (;;) {
		XrEventDataBuffer ev = {XR_TYPE_EVENT_DATA_BUFFER};
		while (xrPollEvent(inst, &ev) == XR_SUCCESS) {
			if (ev.type == XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED) {
				XrEventDataSessionStateChanged *s = (void *)&ev;
				printf("state -> %d\n", s->state);
				if (s->state == XR_SESSION_STATE_READY && !running) {
					XrSessionBeginInfo bi = {XR_TYPE_SESSION_BEGIN_INFO};
					bi.primaryViewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
					CK(xrBeginSession(ses, &bi)); running = 1;
				}
				if (s->state == XR_SESSION_STATE_FOCUSED) focused = 1;
			}
			ev = (XrEventDataBuffer){XR_TYPE_EVENT_DATA_BUFFER};
		}
		clock_gettime(CLOCK_MONOTONIC, &now);
		double el = (now.tv_sec - t0.tv_sec) + (now.tv_nsec - t0.tv_nsec) / 1e9;
		if (el > secs) break;
		if (running) {
			XrActiveActionSet as = {set, XR_NULL_PATH};
			XrActionsSyncInfo syn = {XR_TYPE_ACTIONS_SYNC_INFO}; syn.countActiveActionSets = 1; syn.activeActionSets = &as;
			XrResult r = xrSyncActions(ses, &syn);
			XrTime t = 0; conv(inst, &now, &t);
			if (!printed_profile) {
				XrInteractionProfileState ps = {XR_TYPE_INTERACTION_PROFILE_STATE};
				for (int h = 0; h < 2; h++) if (XR_SUCCEEDED(xrGetCurrentInteractionProfile(ses, hand[h], &ps)) && ps.interactionProfile) {
					char buf[128]; uint32_t n; xrPathToString(inst, ps.interactionProfile, sizeof buf, &n, buf);
					printf("hand %d profile: %s\n", h, buf); printed_profile = 1;
				}
			}
			printf("t=%4.1f sync=%d focused=%d", el, (int)r, focused);
			for (int h = 0; h < 2; h++) {
				XrActionStateGetInfo g = {XR_TYPE_ACTION_STATE_GET_INFO}; g.subactionPath = hand[h];
				XrActionStateFloat f = {XR_TYPE_ACTION_STATE_FLOAT};
				XrActionStateBoolean bo = {XR_TYPE_ACTION_STATE_BOOLEAN};
				XrActionStateVector2f v = {XR_TYPE_ACTION_STATE_VECTOR2F};
				float tr = 0, gr = 0, sx = 0, sy = 0; int b1 = 0, b2 = 0, sc = 0, mn = 0, act = 0;
				g.action = trig; xrGetActionStateFloat(ses, &g, &f); tr = f.currentState; act = f.isActive;
				g.action = grip; xrGetActionStateFloat(ses, &g, &f); gr = f.currentState;
				g.action = btn1; xrGetActionStateBoolean(ses, &g, &bo); b1 = bo.currentState;
				g.action = btn2; xrGetActionStateBoolean(ses, &g, &bo); b2 = bo.currentState;
				g.action = sclick; xrGetActionStateBoolean(ses, &g, &bo); sc = bo.currentState;
				g.action = menu; xrGetActionStateBoolean(ses, &g, &bo); mn = bo.currentState;
				g.action = stick; xrGetActionStateVector2f(ses, &g, &v); sx = v.currentState.x; sy = v.currentState.y;
				XrSpaceLocation loc = {XR_TYPE_SPACE_LOCATION};
				xrLocateSpace(aimsp[h], local, t, &loc);
				printf(" | %c act=%d trig=%.2f grip=%.2f %s=%d %s=%d stick=(%.2f,%.2f) click=%d menu=%d pos=(%.2f %.2f %.2f) q=(%.2f %.2f %.2f %.2f) pf=0x%x",
				       h ? 'R' : 'L', act, tr, gr, h ? "A" : "X", b1, h ? "B" : "Y", b2, sx, sy, sc, mn,
				       loc.pose.position.x, loc.pose.position.y, loc.pose.position.z, loc.pose.orientation.x, loc.pose.orientation.y, loc.pose.orientation.z, loc.pose.orientation.w, (unsigned)loc.locationFlags);
			}
			printf("\n"); fflush(stdout);
		}
		nanosleep(&sl, NULL);
	}
	if (running) xrEndSession(ses);
	xrDestroyInstance(inst);
	return 0;
}
