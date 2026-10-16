// OpenVR smoke test (no graphics): VR_Init -> IVRSystem queries. Build: see steamvr-games.md
#include <openvr.h>
#include <cstdio>
using namespace vr;
int main() {
  EVRInitError e = VRInitError_None;
  IVRSystem *s = VR_Init(&e, VRApplication_Scene);
  if (e != VRInitError_None || !s) { printf("VR_Init failed: %s\n", VR_GetVRInitErrorAsEnglishDescription(e)); return 1; }
  uint32_t w, h; s->GetRecommendedRenderTargetSize(&w, &h);
  printf("VR_Init OK. recommended target %ux%u\n", w, h);
  char buf[256];
  s->GetStringTrackedDeviceProperty(k_unTrackedDeviceIndex_Hmd, Prop_ModelNumber_String, buf, sizeof buf);
  printf("HMD model: %s\n", buf);
  HmdMatrix44_t p = s->GetProjectionMatrix(Eye_Left, 0.1f, 100.f);
  printf("proj[0][0]=%f proj[1][1]=%f\n", p.m[0][0], p.m[1][1]);
  printf("IsTrackedDeviceConnected(hmd)=%d\n", s->IsTrackedDeviceConnected(k_unTrackedDeviceIndex_Hmd));
  VR_Shutdown();
  return 0;
}
