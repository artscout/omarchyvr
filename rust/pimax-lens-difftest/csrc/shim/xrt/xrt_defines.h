/* Stand-in for Monado's xrt/xrt_defines.h: only what pimax_lens.[ch] uses. */
#pragma once
#include <stdbool.h>
#include <stdint.h>

struct xrt_vec2 { float x, y; };
struct xrt_uv_triplet { struct xrt_vec2 r, g, b; };
struct xrt_fov { float angle_left, angle_right, angle_up, angle_down; };
typedef enum { XRT_SUCCESS = 0 } xrt_result_t;

enum xrt_visibility_mask_type
{
	XRT_VISIBILITY_MASK_TYPE_HIDDEN_TRIANGLE_MESH = 1,
	XRT_VISIBILITY_MASK_TYPE_VISIBLE_TRIANGLE_MESH = 2,
	XRT_VISIBILITY_MASK_TYPE_LINE_LOOP = 3,
};
