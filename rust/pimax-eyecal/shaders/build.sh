#!/bin/sh
# Regenerate the checked-in SPIR-V (needs glslc from shaderc / the Vulkan SDK). Run after editing marks.vert / marks.frag.
set -e
cd "$(dirname "$0")"
glslc -O --target-env=vulkan1.0 marks.vert -o marks.vert.spv
glslc -O --target-env=vulkan1.0 marks.frag -o marks.frag.spv
echo "regenerated marks.vert.spv marks.frag.spv"
