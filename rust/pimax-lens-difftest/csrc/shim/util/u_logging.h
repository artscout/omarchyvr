/* Stand-in for Monado's util/u_logging.h: the lines are captured so the test can compare them with the Rust ones. */
#pragma once
void shim_log(int level, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
#define U_LOG_I(...) shim_log(0, __VA_ARGS__)
#define U_LOG_W(...) shim_log(1, __VA_ARGS__)
#define U_LOG_E(...) shim_log(2, __VA_ARGS__)
