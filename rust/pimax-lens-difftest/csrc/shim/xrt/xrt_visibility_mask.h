/* Stand-in for Monado's xrt/xrt_visibility_mask.h (same layout and helpers). */
#pragma once
#include <stddef.h>
#include "xrt/xrt_defines.h"

struct xrt_visibility_mask
{
	enum xrt_visibility_mask_type type;
	uint32_t index_count;
	uint32_t vertex_count;
};

static inline uint32_t *
xrt_visibility_mask_get_indices(const struct xrt_visibility_mask *mask)
{
	return (uint32_t *)&mask[1];
}

static inline struct xrt_vec2 *
xrt_visibility_mask_get_vertices(const struct xrt_visibility_mask *mask)
{
	const uint32_t *indices = xrt_visibility_mask_get_indices(mask);
	return (struct xrt_vec2 *)&indices[mask->index_count];
}
