#include <stddef.h>
#include "pimax.h"
size_t shim_sizeof_imu(void) { return sizeof(pimax_imu_sample); }
size_t shim_sizeof_ctrl(void) { return sizeof(pimax_controller_report); }
size_t shim_off_ctrl_battery(void) { return offsetof(pimax_controller_report, battery); }
size_t shim_off_ctrl_raw(void) { return offsetof(pimax_controller_report, raw); }
size_t shim_off_imu_ipd(void) { return offsetof(pimax_imu_sample, ipd_mm); }
