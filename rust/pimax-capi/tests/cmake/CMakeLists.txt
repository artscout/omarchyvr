# Miniature of ~/src/monado/src/xrt/drivers/CMakeLists.txt (pimax block): a static driver library that is linked into an
# executable ("monado-service"), once with the C sources and once with the Rust staticlib. Same driver source in both.
cmake_minimum_required(VERSION 3.16)
project(pimax_link_check C)
option(PIMAX_USE_RUST_LIBPIMAX "Link libpimax_capi.a instead of compiling libpimax/src/*.c" OFF)
set(PIMAX_LIBPIMAX_DIR "" CACHE PATH "Path to libpimax")
set(PIMAX_RUST_TARGET_DIR "" CACHE PATH "Directory with libpimax_capi.a")
find_package(Threads REQUIRED)

set(DRV_SOURCES driver.c)
if(PIMAX_USE_RUST_LIBPIMAX)
	add_library(drv_pimax STATIC ${DRV_SOURCES})
	target_link_libraries(drv_pimax PRIVATE ${PIMAX_RUST_TARGET_DIR}/libpimax_capi.a ${CMAKE_DL_LIBS} m rt util Threads::Threads)
else()
	add_library(drv_pimax STATIC ${DRV_SOURCES} ${PIMAX_LIBPIMAX_DIR}/src/pimax.c ${PIMAX_LIBPIMAX_DIR}/src/decode.c)
	target_link_libraries(drv_pimax PRIVATE Threads::Threads)
endif()
target_include_directories(drv_pimax PRIVATE ${PIMAX_LIBPIMAX_DIR}/include)

add_executable(service main.c)
target_link_libraries(service PRIVATE drv_pimax)
