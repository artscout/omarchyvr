//! The `swim` pattern on the GPU: one full-screen fragment-shader pass per eye (`shaders/swim.frag`, the shader
//! twin of `patterns/swim.rs`) drawn straight into the swapchain image. Per frame only 128 bytes of push constants
//! (view rotation and position, FOV tangents, tuning warp) change; nothing is rendered or uploaded by the CPU.
//! The HUD text stays CPU-rendered but is a strip texture that is uploaded only when the text changes.
//!
//! The SPIR-V is checked in next to its GLSL source; `shaders/build.sh` regenerates it with `glslc`.

use crate::image::Image;
use crate::patterns::Params;
use ash::vk;

const VERT_SPV: &[u8] = include_bytes!("../shaders/swim.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../shaders/swim.frag.spv");

/// Number of f32 in the push constant block (8 vec4 = 128 bytes, the minimum every device guarantees).
pub const PC_FLOATS: usize = 32;

/// The push constants of one eye (layout of `PC` in `swim.frag`). `hud_rows` = (first row, end row) of the HUD strip
/// in the eye image (end <= first: no HUD); `blank` draws a black image (the eye that is switched off).
pub fn push_constants(p: &Params, blank: bool, hud_rows: (i32, i32)) -> [f32; PC_FLOATS] {
    let mut o = [0.0f32; PC_FLOATS];
    for (i, row) in p.view.rot.iter().enumerate() {
        o[i * 4..i * 4 + 3].copy_from_slice(row);
    }
    o[12..15].copy_from_slice(&p.view.pos);
    o[16..20].copy_from_slice(&p.fov.tangents());
    o[20..24].copy_from_slice(&[p.warp.s, p.warp.dk, p.warp.g, p.warp.dk2]);
    o[24..28].copy_from_slice(&[
        p.w as f32,
        p.h as f32,
        p.eye as f32,
        p.fov.tangent_per_pixel(p.w),
    ]);
    o[28..31].copy_from_slice(&[hud_rows.0 as f32, hud_rows.1 as f32, f32::from(blank)]);
    o
}

fn spv(bytes: &[u8]) -> Result<Vec<u32>, String> {
    ash::util::read_spv(&mut std::io::Cursor::new(bytes)).map_err(|e| format!("SPIR-V: {e}"))
}

fn mem_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32, String> {
    crate::xr::find_memory_type(
        &props.memory_types[..props.memory_type_count as usize],
        bits,
        flags,
    )
    .ok_or_else(|| format!("no memory type with {flags:?}"))
}

/// A texture holding the HUD strip of one eye (full image width, `cap` rows), sampled by the shader.
struct HudTex {
    image: vk::Image,
    view: vk::ImageView,
    mem: vk::DeviceMemory,
    set: vk::DescriptorSet,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    cap: u32,
}

pub struct SwimGpu {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    desc_pool: vk::DescriptorPool,
    hud: Vec<HudTex>,
    /// Per eye, per swapchain image: view and framebuffer.
    targets: Vec<Vec<(vk::ImageView, vk::Framebuffer)>>,
    size: (u32, u32),
}

impl SwimGpu {
    /// Build the pass for images of `format` and `size`; `images[eye]` are that eye's swapchain images (they need
    /// COLOR_ATTACHMENT usage).
    ///
    /// # Safety
    /// `device`, `queue` (of family `qf`) and the images must be valid and stay alive until [`SwimGpu::destroy`].
    pub unsafe fn new(
        device: &ash::Device,
        queue: vk::Queue,
        qf: u32,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        size: (u32, u32),
        images: [&[vk::Image]; 2],
    ) -> Result<SwimGpu, String> {
        let e = |what: &str| {
            let what = what.to_string();
            move |r: vk::Result| format!("{what}: {r:?}")
        };
        let vert = spv(VERT_SPV)?;
        let frag = spv(FRAG_SPV)?;
        let vs = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vert), None)
            .map_err(e("vertex shader"))?;
        let fs = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&frag), None)
            .map_err(e("fragment shader"))?;
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .map_err(e("descriptor set layout"))?;
        let set_layouts = [set_layout];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size((PC_FLOATS * 4) as u32)];
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )
            .map_err(e("pipeline layout"))?;
        let attachments = [vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let refs = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&refs)];
        let deps = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::ALL_COMMANDS)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
        let render_pass = device
            .create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&deps),
                None,
            )
            .map_err(e("render pass"))?;
        let main = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs)
                .name(main),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(main),
        ];
        let vin = vk::PipelineVertexInputStateCreateInfo::default();
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: size.0 as f32,
            height: size.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }];
        let scissors = [vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: size.0,
                height: size.1,
            },
        }];
        let vp = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rs = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let ms = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_att = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_att);
        let pipeline = device
            .create_graphics_pipelines(
                vk::PipelineCache::null(),
                &[vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vin)
                    .input_assembly_state(&ia)
                    .viewport_state(&vp)
                    .rasterization_state(&rs)
                    .multisample_state(&ms)
                    .color_blend_state(&blend)
                    .layout(layout)
                    .render_pass(render_pass)
                    .subpass(0)],
                None,
            )
            .map_err(|(_, r)| format!("graphics pipeline: {r:?}"))?[0];
        device.destroy_shader_module(vs, None);
        device.destroy_shader_module(fs, None);
        let sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
            .map_err(e("sampler"))?;
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 2,
        }];
        let desc_pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&pool_sizes),
                None,
            )
            .map_err(e("descriptor pool"))?;
        let sets = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(desc_pool)
                    .set_layouts(&[set_layout, set_layout]),
            )
            .map_err(e("descriptor sets"))?;
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qf)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(e("command pool"))?;
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .map_err(e("command buffer"))?[0];
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(e("fence"))?;

        let mut targets = Vec::new();
        for eye_images in images {
            let mut v = Vec::new();
            for &img in eye_images {
                let view = device
                    .create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(img)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(format)
                            .subresource_range(color_range()),
                        None,
                    )
                    .map_err(e("swapchain image view"))?;
                let views = [view];
                let fb = device
                    .create_framebuffer(
                        &vk::FramebufferCreateInfo::default()
                            .render_pass(render_pass)
                            .attachments(&views)
                            .width(size.0)
                            .height(size.1)
                            .layers(1),
                        None,
                    )
                    .map_err(e("framebuffer"))?;
                v.push((view, fb));
            }
            targets.push(v);
        }

        let mut me = SwimGpu {
            device: device.clone(),
            queue,
            pool,
            cmd,
            fence,
            render_pass,
            set_layout,
            layout,
            pipeline,
            sampler,
            desc_pool,
            hud: Vec::new(),
            targets,
            size,
        };
        let cap = size.1.min(1600);
        for &set in &sets {
            let t = me.make_hud_tex(mem_props, size.0, cap, set)?;
            me.hud.push(t);
        }
        for eye in 0..2 {
            me.set_hud(eye, None)?;
        }
        Ok(me)
    }

    unsafe fn make_hud_tex(
        &self,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        w: u32,
        cap: u32,
        set: vk::DescriptorSet,
    ) -> Result<HudTex, String> {
        let d = &self.device;
        let e = |what: &str| {
            let what = what.to_string();
            move |r: vk::Result| format!("{what}: {r:?}")
        };
        let image = d
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: w,
                        height: cap,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(e("hud image"))?;
        let req = d.get_image_memory_requirements(image);
        let mem = d
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mem_type(
                        mem_props,
                        req.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )
            .map_err(e("hud memory"))?;
        d.bind_image_memory(image, mem, 0).map_err(e("hud bind"))?;
        let view = d
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(color_range()),
                None,
            )
            .map_err(e("hud view"))?;
        let len = w as u64 * cap as u64 * 4;
        let staging = d
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(len)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )
            .map_err(e("hud staging"))?;
        let sreq = d.get_buffer_memory_requirements(staging);
        let staging_mem = d
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(sreq.size)
                    .memory_type_index(mem_type(
                        mem_props,
                        sreq.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )?),
                None,
            )
            .map_err(e("hud staging memory"))?;
        d.bind_buffer_memory(staging, staging_mem, 0)
            .map_err(e("hud staging bind"))?;
        let staging_ptr = d
            .map_memory(staging_mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(e("hud staging map"))? as *mut u8;
        let info = [vk::DescriptorImageInfo {
            sampler: self.sampler,
            image_view: view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        d.update_descriptor_sets(
            &[vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&info)],
            &[],
        );
        Ok(HudTex {
            image,
            view,
            mem,
            set,
            staging,
            staging_mem,
            staging_ptr,
            cap,
        })
    }

    unsafe fn submit_wait(&self) -> Result<(), String> {
        let d = &self.device;
        d.end_command_buffer(self.cmd)
            .map_err(|r| format!("end: {r:?}"))?;
        let cmds = [self.cmd];
        d.queue_submit(
            self.queue,
            &[vk::SubmitInfo::default().command_buffers(&cmds)],
            self.fence,
        )
        .map_err(|r| format!("submit: {r:?}"))?;
        d.wait_for_fences(&[self.fence], true, u64::MAX)
            .map_err(|r| format!("wait: {r:?}"))?;
        d.reset_fences(&[self.fence])
            .map_err(|r| format!("reset fence: {r:?}"))
    }

    unsafe fn begin(&self) -> Result<(), String> {
        let d = &self.device;
        d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
            .map_err(|r| format!("reset: {r:?}"))?;
        d.begin_command_buffer(
            self.cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .map_err(|r| format!("begin: {r:?}"))
    }

    /// Replace the HUD strip of `eye` (`None`: empty). The strip image is as wide as the eye image and is placed
    /// at row `top`; pixels that are exactly black are transparent. Rows beyond the texture capacity are cut.
    /// Returns the row range (first, end) to pass in the push constants.
    pub fn set_hud(&self, eye: usize, strip: Option<(&Image, i32)>) -> Result<(i32, i32), String> {
        // SAFETY: the texture, staging buffer and command objects belong to `self` and are idle (every call waits).
        unsafe {
            let t = &self.hud[eye];
            let d = &self.device;
            self.begin()?;
            let barrier = |old, new, src, dst| {
                vk::ImageMemoryBarrier::default()
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(t.image)
                    .subresource_range(color_range())
                    .old_layout(old)
                    .new_layout(new)
                    .src_access_mask(src)
                    .dst_access_mask(dst)
            };
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::TRANSFER_WRITE,
                )],
            );
            let mut rows = (0, 0);
            if let Some((img, top)) = strip {
                let n = img.h.min(t.cap);
                let w = img.w.min(self.size.0) as usize;
                let dst = std::slice::from_raw_parts_mut(
                    t.staging_ptr,
                    self.size.0 as usize * t.cap as usize * 4,
                );
                for y in 0..n as usize {
                    let row = &mut dst[y * self.size.0 as usize * 4..][..self.size.0 as usize * 4];
                    row.fill(0);
                    for x in 0..w {
                        let p = img.get(x as u32, y as u32);
                        row[x * 4..x * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
                    }
                }
                d.cmd_copy_buffer_to_image(
                    self.cmd,
                    t.staging,
                    t.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[vk::BufferImageCopy {
                        buffer_offset: 0,
                        buffer_row_length: self.size.0,
                        buffer_image_height: 0,
                        image_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        },
                        image_offset: vk::Offset3D::default(),
                        image_extent: vk::Extent3D {
                            width: self.size.0,
                            height: n,
                            depth: 1,
                        },
                    }],
                );
                rows = (top, top + n as i32);
            }
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::TRANSFER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                )],
            );
            self.submit_wait()?;
            Ok(rows)
        }
    }

    /// Draw the eye image into swapchain image `idx` of `eye` and wait for the GPU; the image is left in
    /// COLOR_ATTACHMENT_OPTIMAL as OpenXR wants.
    pub fn draw(&self, eye: usize, idx: usize, pc: &[f32; PC_FLOATS]) -> Result<(), String> {
        // SAFETY: the framebuffer, pipeline and descriptor set belong to `self`; the image was acquired by the
        // caller; the submission is waited for before returning.
        unsafe {
            let d = &self.device;
            self.begin()?;
            let (_, fb) = self.targets[eye][idx];
            d.cmd_begin_render_pass(
                self.cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(fb)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: self.size.0,
                            height: self.size.1,
                        },
                    }),
                vk::SubpassContents::INLINE,
            );
            d.cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            d.cmd_bind_descriptor_sets(
                self.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.hud[eye].set],
                &[],
            );
            let bytes = std::slice::from_raw_parts(pc.as_ptr().cast::<u8>(), PC_FLOATS * 4);
            d.cmd_push_constants(
                self.cmd,
                self.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes,
            );
            d.cmd_draw(self.cmd, 3, 1, 0, 0);
            d.cmd_end_render_pass(self.cmd);
            self.submit_wait()
        }
    }

    /// Free everything (the caller has waited for the device to be idle).
    pub fn destroy(&mut self) {
        // SAFETY: handles created in `new`, destroyed once; nothing is in flight (every call waits on its fence).
        unsafe {
            let d = &self.device;
            for t in self.hud.drain(..) {
                d.unmap_memory(t.staging_mem);
                d.destroy_buffer(t.staging, None);
                d.free_memory(t.staging_mem, None);
                d.destroy_image_view(t.view, None);
                d.destroy_image(t.image, None);
                d.free_memory(t.mem, None);
            }
            for (view, fb) in self.targets.drain(..).flatten() {
                d.destroy_framebuffer(fb, None);
                d.destroy_image_view(view, None);
            }
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.pool, None);
            d.destroy_descriptor_pool(self.desc_pool, None);
            d.destroy_sampler(self.sampler, None);
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{rot_y, Fov};
    use crate::patterns::{self, Pattern};
    use crate::tune::Tune;

    #[test]
    fn push_constants_layout() {
        let mut p = Params::new(
            200,
            100,
            Fov::from_half_angles_deg(45.0, 45.0, 30.0, 30.0),
            1,
        );
        p.view.pos = [1.0, 2.0, 3.0];
        p.warp = Tune {
            dk: 2,
            sc: 0,
            dk2: 3,
        }
        .warp(0.65);
        let pc = push_constants(&p, true, (10, 20));
        assert_eq!(&pc[0..3], &[1.0, 0.0, 0.0]);
        assert_eq!(&pc[12..15], &[1.0, 2.0, 3.0]);
        assert!((pc[16] - 1.0).abs() < 1e-5 && (pc[18] - 30f32.to_radians().tan()).abs() < 1e-5);
        assert_eq!(&pc[20..24], &[1.0, 0.02, 0.65, 0.03]);
        assert_eq!(&pc[24..27], &[200.0, 100.0, 1.0]);
        assert_eq!(&pc[28..31], &[10.0, 20.0, 1.0]);
        assert_eq!(PC_FLOATS * 4, 128);
    }

    /// The shader and the CPU renderer draw the same picture. Needs a Vulkan device (any, llvmpipe is fine);
    /// skipped (with a note) when there is none.
    #[test]
    fn shader_matches_the_cpu_renderer() {
        // SAFETY: plain headless Vulkan: own instance, device, image and buffers, all destroyed at the end.
        unsafe {
            let Ok(entry) = ash::Entry::load() else {
                eprintln!("skipped: no Vulkan loader");
                return;
            };
            let Ok(instance) = entry.create_instance(&vk::InstanceCreateInfo::default(), None)
            else {
                eprintln!("skipped: cannot create a Vulkan instance");
                return;
            };
            let found = instance
                .enumerate_physical_devices()
                .unwrap_or_default()
                .into_iter()
                .find_map(|pd| {
                    instance
                        .get_physical_device_queue_family_properties(pd)
                        .iter()
                        .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                        .map(|qf| (pd, qf as u32))
                });
            let Some((pd, qf)) = found else {
                eprintln!("skipped: no Vulkan device with a graphics queue");
                instance.destroy_instance(None);
                return;
            };
            let prio = [1.0f32];
            let qinfo = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(qf)
                .queue_priorities(&prio)];
            let device = instance
                .create_device(
                    pd,
                    &vk::DeviceCreateInfo::default().queue_create_infos(&qinfo),
                    None,
                )
                .expect("device");
            let queue = device.get_device_queue(qf, 0);
            let mp = instance.get_physical_device_memory_properties(pd);

            let (w, h) = (320u32, 300u32);
            let format = vk::Format::R8G8B8A8_SRGB;
            let target = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D {
                            width: w,
                            height: h,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        ),
                    None,
                )
                .unwrap();
            let req = device.get_image_memory_requirements(target);
            let tmem = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(
                            mem_type(
                                &mp,
                                req.memory_type_bits,
                                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                            )
                            .unwrap(),
                        ),
                    None,
                )
                .unwrap();
            device.bind_image_memory(target, tmem, 0).unwrap();
            let mut gpu = SwimGpu::new(
                &device,
                queue,
                qf,
                &mp,
                format,
                (w, h),
                [&[target], &[target]],
            )
            .expect("swim pass");

            let fov = Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5);
            let mut p = Params::new(w, h, fov, 1);
            p.view.rot = rot_y(17.0);
            p.view.pos = [0.12, -0.05, 0.3];
            p.warp = Tune {
                dk: 3,
                sc: 2,
                dk2: 4,
            }
            .warp(0.653);
            let lines: Vec<String> = ["SWIM HUD TEST", "SECOND LINE OF TEXT"]
                .map(String::from)
                .to_vec();
            let strip = patterns::hud_strip(w, h, &fov, &lines).unwrap();
            let rows = gpu.set_hud(1, Some((&strip.1, strip.0))).unwrap();
            gpu.draw(1, 0, &push_constants(&p, false, rows)).unwrap();

            // read back
            let len = (w * h * 4) as u64;
            let buf = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(len)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .unwrap();
            let breq = device.get_buffer_memory_requirements(buf);
            let bmem = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(breq.size)
                        .memory_type_index(
                            mem_type(
                                &mp,
                                breq.memory_type_bits,
                                vk::MemoryPropertyFlags::HOST_VISIBLE
                                    | vk::MemoryPropertyFlags::HOST_COHERENT,
                            )
                            .unwrap(),
                        ),
                    None,
                )
                .unwrap();
            device.bind_buffer_memory(buf, bmem, 0).unwrap();
            gpu.begin().unwrap();
            let barrier = vk::ImageMemoryBarrier::default()
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(target)
                .subresource_range(color_range())
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            device.cmd_pipeline_barrier(
                gpu.cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            device.cmd_copy_image_to_buffer(
                gpu.cmd,
                target,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buf,
                &[vk::BufferImageCopy {
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    image_extent: vk::Extent3D {
                        width: w,
                        height: h,
                        depth: 1,
                    },
                    ..Default::default()
                }],
            );
            gpu.submit_wait().unwrap();
            let ptr = device
                .map_memory(bmem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .unwrap() as *const u8;
            let got = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            device.unmap_memory(bmem);

            let cpu = patterns::render(Pattern::Swim, &p, Some(&lines), 4);
            let (mut sum, mut big, mut n) = (0u64, 0usize, 0usize);
            for y in 0..h {
                for x in 0..w {
                    let c = cpu.get(x, y);
                    let g = &got[((y * w + x) * 4) as usize..][..3];
                    for k in 0..3 {
                        let d = (i32::from(c[k]) - i32::from(g[k])).unsigned_abs();
                        sum += u64::from(d);
                        big += usize::from(d > 24);
                        n += 1;
                    }
                }
            }
            let mean = sum as f64 / n as f64;
            let frac = big as f64 / n as f64;
            eprintln!("GPU vs CPU: mean abs diff {mean:.3} codes, {:.4}% of samples differ by more than 24", frac * 100.0);

            gpu.destroy();
            device.destroy_buffer(buf, None);
            device.free_memory(bmem, None);
            device.destroy_image(target, None);
            device.free_memory(tmem, None);
            device.destroy_device(None);
            instance.destroy_instance(None);
            assert!(mean < 1.0, "mean difference {mean}");
            assert!(
                frac < 0.004,
                "{:.3}% of samples differ strongly",
                frac * 100.0
            );
        }
    }
}
