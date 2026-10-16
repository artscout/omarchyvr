#!/bin/sh
# Regenerate the checked-in SPIR-V (needs glslc from shaderc / the Vulkan SDK). Run after editing swim.vert / swim.frag.
set -e
cd "$(dirname "$0")"
glslc -O --target-env=vulkan1.0 swim.vert -o swim.vert.spv
glslc -O --target-env=vulkan1.0 swim.frag -o swim.frag.spv
echo "regenerated swim.vert.spv swim.frag.spv"
