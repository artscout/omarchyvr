//! The scene on the GPU: one full-screen fragment pass per eye (`shaders/marks.frag`) drawn straight into the
//! swapchain image. Per frame only a 1.6 KB uniform block changes (the marks as directions in that eye's view space);
//! nothing is rendered or uploaded by the CPU, so the frame time stays a fraction of a millisecond at 3552 x 3840.
//!
//! [`block`] builds the uniform block (pure, tested), [`shade`] is the CPU twin of the shader (tests).

use crate::procedure::{MarkKind, Scene};
use ash::vk;
use pimax_lenstest::geometry::{mat_vec, transpose, Mat3};

const VERT_SPV: &[u8] = include_bytes!("../shaders/marks.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../shaders/marks.frag.spv");

/// Marks per eye (the shader's `MAX_MARKS`).
pub const MAX_MARKS: usize = 32;
/// Floats in the uniform block: 3 vec4 header + 3 vec4 per mark (std140).
pub const BLOCK_FLOATS: usize = 12 + MAX_MARKS * 12;

/// One eye's view: rotation eye -> head (OpenXR view orientation), position in the head frame, FOV tangents
/// (left, right, up, down; positive) and image size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeView {
    /// Eye-to-head rotation.
    pub rot: Mat3,
    /// Eye position, metres, head frame.
    pub pos: [f32; 3],
    /// Tangents of the half-angles.
    pub tan: [f32; 4],
    /// Image size.
    pub size: (u32, u32),
}

/// The uniform block of one eye.
pub fn block(scene: &Scene, v: &EyeView) -> [f32; BLOCK_FLOATS] {
    let mut b = [0.0f32; BLOCK_FLOATS];
    b[0..4].copy_from_slice(&v.tan);
    let tpp = (v.tan[0] + v.tan[1]) / v.size.0 as f32;
    let inv = transpose(&v.rot);
    let mut n = 0usize;
    for m in &scene.marks {
        if n >= MAX_MARKS {
            break;
        }
        let rel = [
            (m.pos[0] - f64::from(v.pos[0])) as f32,
            (m.pos[1] - f64::from(v.pos[1])) as f32,
            (m.pos[2] - f64::from(v.pos[2])) as f32,
        ];
        let d = mat_vec(&inv, rel);
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if l.is_nan() || l <= 1e-6 || d[2] > -1e-3 * l {
            continue; // behind the eye
        }
        let o = 12 + n * 12;
        b[o..o + 3].copy_from_slice(&[d[0] / l, d[1] / l, d[2] / l]);
        b[o + 3] = m.radius_deg.to_radians() as f32;
        b[o + 4..o + 7].copy_from_slice(&m.color);
        b[o + 7] = match m.kind {
            MarkKind::Disc => 0.0,
            MarkKind::Ring => 1.0,
            MarkKind::Cross => 2.0,
        };
        b[o + 8] = m.width_deg.to_radians() as f32;
        n += 1;
    }
    b[4..8].copy_from_slice(&[v.size.0 as f32, v.size.1 as f32, tpp, n as f32]);
    b[8..11].copy_from_slice(&scene.background);
    b
}

fn cover(d: f32, pix: f32) -> f32 {
    (d / pix + 0.5).clamp(0.0, 1.0)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// CPU twin of `marks.frag`: the sRGB colour of pixel (x, y).
pub fn shade(b: &[f32; BLOCK_FLOATS], x: u32, y: u32) -> [f32; 3] {
    let (w, h) = (b[4], b[5]);
    let (tl, tr, tu, td) = (b[0], b[1], b[2], b[3]);
    let tx = -tl + (x as f32 + 0.5) / w * (tl + tr);
    let ty = tu - (y as f32 + 0.5) / h * (tu + td);
    let r2 = tx * tx + ty * ty;
    let n = (r2 + 1.0).sqrt();
    let v = [tx / n, ty / n, -1.0 / n];
    let pix = b[6] / (1.0 + r2);
    let mut code = [b[8], b[9], b[10]];
    let count = (b[7] + 0.5) as usize;
    for i in 0..count.min(MAX_MARKS) {
        let o = 12 + i * 12;
        let d = [b[o], b[o + 1], b[o + 2]];
        let r = b[o + 3];
        let kind = (b[o + 7] + 0.5) as i32;
        let wd = b[o + 8];
        let c = dot(d, v);
        if c < (r + wd + 4.0 * pix).min(3.0).cos() {
            continue;
        }
        let cr = cross(d, v);
        let ang = dot(cr, cr).sqrt().atan2(c);
        let cov = match kind {
            0 => cover(r - ang, pix),
            1 => cover(0.5 * wd - (ang - r).abs(), pix),
            _ => {
                let e1 = cross(d, [0.0, 1.0, 0.0]);
                let l = dot(e1, e1).sqrt();
                let e1 = [e1[0] / l, e1[1] / l, e1[2] / l];
                let e2 = cross(e1, d);
                let lx = dot(v, e1) / c;
                let ly = dot(v, e2) / c;
                let b1 = cover(0.5 * wd - ly.abs(), pix).min(cover(r - lx.abs(), pix));
                let b2 = cover(0.5 * wd - lx.abs(), pix).min(cover(r - ly.abs(), pix));
                b1.max(b2)
            }
        };
        let col = [b[o + 4], b[o + 5], b[o + 6]];
        for k in 0..3 {
            code[k] += (col[k] - code[k]) * cov;
        }
    }
    code
}

fn spv(bytes: &[u8]) -> Result<Vec<u32>, String> {
    ash::util::read_spv(&mut std::io::Cursor::new(bytes)).map_err(|e| format!("SPIR-V: {e}"))
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

/// The pass: pipeline, one uniform buffer and descriptor set per eye, a framebuffer per swapchain image.
pub struct MarksGpu {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    desc_pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    ubo: Vec<(vk::Buffer, vk::DeviceMemory, *mut f32)>,
    targets: Vec<Vec<(vk::ImageView, vk::Framebuffer)>>,
    size: (u32, u32),
}

impl MarksGpu {
    /// Build the pass for swapchain images of `format` and `size`; `images[eye]` need COLOR_ATTACHMENT usage.
    ///
    /// # Safety
    /// `device`, `queue` (of family `qf`) and the images must be valid until [`MarksGpu::destroy`].
    pub unsafe fn new(
        device: &ash::Device,
        queue: vk::Queue,
        qf: u32,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        size: (u32, u32),
        images: [&[vk::Image]; 2],
    ) -> Result<MarksGpu, String> {
        let e = |what: &'static str| move |r: vk::Result| format!("{what}: {r:?}");
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
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = device
            .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)
            .map_err(e("descriptor set layout"))?;
        let set_layouts = [set_layout];
        let layout = device
            .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts), None)
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
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
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
        let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_att = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
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
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 2,
        }];
        let desc_pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().max_sets(2).pool_sizes(&pool_sizes),
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
        let mut ubo = Vec::new();
        let len = (BLOCK_FLOATS * 4) as u64;
        for &set in &sets {
            let buf = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(len)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(e("uniform buffer"))?;
            let req = device.get_buffer_memory_requirements(buf);
            let mt = pimax_lenstest::xr::find_memory_type(
                &mem_props.memory_types[..mem_props.memory_type_count as usize],
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or("no host visible and coherent memory for the uniform buffer")?;
            let mem = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(mt),
                    None,
                )
                .map_err(e("uniform memory"))?;
            device.bind_buffer_memory(buf, mem, 0).map_err(e("uniform bind"))?;
            let ptr = device
                .map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(e("uniform map"))? as *mut f32;
            let info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: len,
            }];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&info)],
                &[],
            );
            ubo.push((buf, mem, ptr));
        }
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
        Ok(MarksGpu {
            device: device.clone(),
            queue,
            pool,
            cmd,
            fence,
            render_pass,
            set_layout,
            layout,
            pipeline,
            desc_pool,
            sets,
            ubo,
            targets,
            size,
        })
    }

    /// Draw `block` into swapchain image `idx` of `eye` and wait for the GPU.
    pub fn draw(&self, eye: usize, idx: usize, block: &[f32; BLOCK_FLOATS]) -> Result<(), String> {
        // SAFETY: the uniform memory is host-coherent, mapped for BLOCK_FLOATS floats and not in use (every draw
        // waits for its fence); the framebuffer, pipeline and set belong to `self`; the image was acquired by the
        // caller.
        unsafe {
            std::ptr::copy_nonoverlapping(block.as_ptr(), self.ubo[eye].2, BLOCK_FLOATS);
            let d = &self.device;
            d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
                .map_err(|r| format!("reset: {r:?}"))?;
            d.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|r| format!("begin: {r:?}"))?;
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
                &[self.sets[eye]],
                &[],
            );
            d.cmd_draw(self.cmd, 3, 1, 0, 0);
            d.cmd_end_render_pass(self.cmd);
            d.end_command_buffer(self.cmd).map_err(|r| format!("end: {r:?}"))?;
            let cmds = [self.cmd];
            d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)
                .map_err(|r| format!("submit: {r:?}"))?;
            d.wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(|r| format!("wait: {r:?}"))?;
            d.reset_fences(&[self.fence]).map_err(|r| format!("reset fence: {r:?}"))
        }
    }

    /// Free everything (the caller has waited for the device to be idle).
    pub fn destroy(&mut self) {
        // SAFETY: nothing is in flight (every draw waits); each handle was created in `new` and is destroyed once.
        unsafe {
            let d = &self.device;
            for (view, fb) in self.targets.drain(..).flatten() {
                d.destroy_framebuffer(fb, None);
                d.destroy_image_view(view, None);
            }
            for (b, m, _) in self.ubo.drain(..) {
                d.unmap_memory(m);
                d.destroy_buffer(b, None);
                d.free_memory(m, None);
            }
            d.destroy_descriptor_pool(self.desc_pool, None);
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_render_pass(self.render_pass, None);
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.pool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedure::{Mark, BACKGROUND};
    use pimax_lenstest::geometry::{quat_to_mat, IDENT3};

    fn view(eye: usize, cant_deg: f32) -> EyeView {
        let c = (if eye == 0 { cant_deg } else { -cant_deg }).to_radians() * 0.5;
        EyeView {
            rot: if cant_deg == 0.0 { IDENT3 } else { quat_to_mat([0.0, c.sin(), 0.0, c.cos()]) },
            pos: [if eye == 0 { -0.032 } else { 0.032 }, 0.0, 0.0],
            tan: [1.0, 1.0, 0.9, 0.9],
            size: (400, 360),
        }
    }

    fn scene(marks: Vec<Mark>) -> Scene {
        Scene { marks, hud: vec![], background: BACKGROUND }
    }

    fn centroid(b: &[f32; BLOCK_FLOATS], w: u32, h: u32) -> (f32, f32, f32) {
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        for y in 0..h {
            for x in 0..w {
                let c = shade(b, x, y);
                let v = (c[0] - BACKGROUND[0]).max(0.0);
                sx += v * (x as f32 + 0.5);
                sy += v * (y as f32 + 0.5);
                sw += v;
            }
        }
        (sx / sw, sy / sw, sw)
    }

    #[test]
    fn a_disc_lands_where_each_eye_must_see_it_also_with_canted_views() {
        let p = crate::layout::Target::new(12.0, -9.0).point();
        let m = Mark { pos: p, kind: MarkKind::Disc, radius_deg: 1.0, width_deg: 0.0, color: [1.0; 3] };
        for cant in [0.0, 2.0] {
            for eye in 0..2 {
                let v = view(eye, cant);
                let b = block(&scene(vec![m]), &v);
                let (cx, cy, _) = centroid(&b, 400, 360);
                // expected: the target in this eye's view space, projected
                let inv = transpose(&v.rot);
                let rel = [(p[0] - f64::from(v.pos[0])) as f32, p[1] as f32, p[2] as f32];
                let d = mat_vec(&inv, rel);
                let (tx, ty) = (d[0] / -d[2], d[1] / -d[2]);
                let ex = (tx + 1.0) / 2.0 * 400.0;
                let ey = (0.9 - ty) / 1.8 * 360.0;
                assert!((cx - ex).abs() < 0.3 && (cy - ey).abs() < 0.3, "cant {cant} eye {eye}: {cx},{cy} vs {ex},{ey}");
            }
        }
    }

    #[test]
    fn abc_target_has_a_cut_cross_and_a_centre_dot() {
        let p = [0.0, 0.0, -2.0];
        let marks = vec![
            Mark { pos: p, kind: MarkKind::Disc, radius_deg: 8.0, width_deg: 0.0, color: [1.0; 3] },
            Mark { pos: p, kind: MarkKind::Cross, radius_deg: 8.5, width_deg: 1.5, color: BACKGROUND },
            Mark { pos: p, kind: MarkKind::Disc, radius_deg: 1.0, width_deg: 0.0, color: [1.0, 0.0, 0.0] },
        ];
        let v = EyeView { pos: [0.0; 3], ..view(0, 0.0) };
        let b = block(&scene(marks), &v);
        let (cx, cy) = (200u32, 180u32);
        assert_eq!(shade(&b, cx, cy), [1.0, 0.0, 0.0], "red centre");
        // on the horizontal bar, 4 degrees out: background; between the bars: white
        let px_per_deg = 400.0 / 2.0 * 1f32.to_radians();
        let bar = shade(&b, cx + (4.0 / px_per_deg.recip()) as u32, cy);
        assert!((bar[0] - BACKGROUND[0]).abs() < 0.02, "{bar:?}");
        let off = (4.0 * px_per_deg) as u32;
        let quad = shade(&b, cx + off, cy - off);
        assert!(quad[1] > 0.95, "{quad:?}");
        // outside the disc: background
        let out = shade(&b, cx + (12.0 * px_per_deg) as u32, cy);
        assert_eq!(out, BACKGROUND);
    }

    #[test]
    fn rings_are_hollow_and_marks_behind_are_dropped() {
        let ring = Mark { pos: [0.0, 0.0, -2.0], kind: MarkKind::Ring, radius_deg: 5.0, width_deg: 0.5, color: [0.0, 1.0, 0.0] };
        let behind = Mark { pos: [0.0, 0.0, 2.0], ..ring };
        let v = EyeView { pos: [0.0; 3], ..view(0, 0.0) };
        let b = block(&scene(vec![ring, behind]), &v);
        assert_eq!(b[7], 1.0, "only the ring is in the block");
        assert_eq!(shade(&b, 200, 180), BACKGROUND);
        let px_per_deg = 400.0 / 2.0 * 1f32.to_radians();
        let on = shade(&b, 200 + (5.0 * px_per_deg) as u32, 180);
        assert!(on[1] > 0.5, "{on:?}");
        let many: Vec<Mark> = (0..40).map(|_| ring).collect();
        assert_eq!(block(&scene(many), &v)[7] as usize, MAX_MARKS);
    }
}
