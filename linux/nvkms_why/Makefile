# NVSRC: a checkout of https://github.com/NVIDIA/open-gpu-kernel-modules at the driver version in use
NVSRC ?= $(HOME)/src/open-gpu-kernel-modules
INC = -I$(NVSRC)/src/nvidia-modeset/interface -I$(NVSRC)/src/common/sdk/nvidia/inc -I$(NVSRC)/src/common/inc -I$(NVSRC)/src/common/unix/common/inc -I$(NVSRC)/src/common/sdk/nvidia/inc/class
nvkms_why: nvkms_why.c
	gcc -O1 -Wall -std=gnu11 $(INC) -o $@ $<
dpcd_read: dpcd_read.c
	gcc -O1 -Wall -std=gnu11 $(INC) -o $@ $<
