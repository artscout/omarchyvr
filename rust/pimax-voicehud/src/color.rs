//! Swapchain formats and the byte encoding of the texture.
//!
//! The texture is authored as straight (unpremultiplied) alpha with sRGB-coded colour bytes. Monado's compositor
//! samples an `*_SRGB` swapchain through an sRGB view (decoded to linear, blended in linear light, encoded again): the
//! bytes are uploaded unchanged. An `*_UNORM` swapchain is sampled as linear, so the colour must be stored as its
//! linear value for the panel to receive the intended code. Alpha is always linear and never converted.

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// How the compositor treats the bytes of the swapchain format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// `*_SRGB`: colour bytes are sRGB codes (uploaded as authored).
    Srgb,
    /// `*_UNORM`: colour bytes are linear light.
    Linear,
}

impl Encoding {
    /// Byte stored for the colour code `code` (0..=255).
    pub fn byte(self, code: u8) -> u8 {
        match self {
            Encoding::Srgb => code,
            Encoding::Linear => {
                (srgb_to_linear(f32::from(code) / 255.0) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            }
        }
    }

    /// 256-entry table: authored code byte -> stored byte.
    pub fn table(self) -> [u8; 256] {
        let mut t = [0u8; 256];
        for (i, e) in t.iter_mut().enumerate() {
            *e = self.byte(i as u8);
        }
        t
    }
}

pub const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;
pub const VK_FORMAT_B8G8R8A8_UNORM: u32 = 44;
pub const VK_FORMAT_B8G8R8A8_SRGB: u32 = 50;

/// Pick a swapchain format from the ones the runtime offers: sRGB first (bytes reach the compositor unchanged), then
/// UNORM; `prefer_unorm` swaps the two. RGBA before BGRA.
pub fn choose_format(offered: &[u32], prefer_unorm: bool) -> Option<u32> {
    let order: [u32; 4] = if prefer_unorm {
        [
            VK_FORMAT_R8G8B8A8_UNORM,
            VK_FORMAT_B8G8R8A8_UNORM,
            VK_FORMAT_R8G8B8A8_SRGB,
            VK_FORMAT_B8G8R8A8_SRGB,
        ]
    } else {
        [
            VK_FORMAT_R8G8B8A8_SRGB,
            VK_FORMAT_B8G8R8A8_SRGB,
            VK_FORMAT_R8G8B8A8_UNORM,
            VK_FORMAT_B8G8R8A8_UNORM,
        ]
    };
    order.into_iter().find(|f| offered.contains(f))
}

pub fn encoding_of(vk_format: u32) -> Encoding {
    match vk_format {
        VK_FORMAT_R8G8B8A8_SRGB | VK_FORMAT_B8G8R8A8_SRGB => Encoding::Srgb,
        _ => Encoding::Linear,
    }
}

pub fn is_bgra(vk_format: u32) -> bool {
    matches!(
        vk_format,
        VK_FORMAT_B8G8R8A8_SRGB | VK_FORMAT_B8G8R8A8_UNORM
    )
}

pub fn format_name(vk_format: u32) -> String {
    match vk_format {
        VK_FORMAT_R8G8B8A8_UNORM => "R8G8B8A8_UNORM".into(),
        VK_FORMAT_R8G8B8A8_SRGB => "R8G8B8A8_SRGB".into(),
        VK_FORMAT_B8G8R8A8_UNORM => "B8G8R8A8_UNORM".into(),
        VK_FORMAT_B8G8R8A8_SRGB => "B8G8R8A8_SRGB".into(),
        other => format!("VkFormat {other}"),
    }
}

/// Convert the authored RGBA pixels (straight alpha, sRGB codes) into the bytes of the swapchain format: colour per
/// [`Encoding`], channel order for BGRA, alpha untouched. `src` and `dst` must have the same length.
pub fn encode_into(src: &[u8], dst: &mut [u8], enc: Encoding, bgra: bool) {
    assert_eq!(src.len(), dst.len(), "pixel buffers differ in size");
    let table = enc.table();
    let (src_px, _) = src.as_chunks::<4>();
    let (dst_px, _) = dst.as_chunks_mut::<4>();
    for (s, d) in src_px.iter().zip(dst_px.iter_mut()) {
        let (r, g, b) = (
            table[s[0] as usize],
            table[s[1] as usize],
            table[s[2] as usize],
        );
        if bgra {
            d[0] = b;
            d[1] = g;
            d[2] = r;
        } else {
            d[0] = r;
            d[1] = g;
            d[2] = b;
        }
        d[3] = s[3];
    }
}

/// [`encode_into`] for a whole swapchain format.
pub fn encode_for_format(src: &[u8], dst: &mut [u8], vk_format: u32) {
    encode_into(src, dst, encoding_of(vk_format), is_bgra(vk_format));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip_and_known_values() {
        for i in 0..=255 {
            let c = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-5, "{i}");
        }
        assert!((srgb_to_linear(0.5) - 0.214).abs() < 1e-3);
        assert!((linear_to_srgb(0.5) - 0.7354).abs() < 1e-3);
    }

    #[test]
    fn srgb_format_keeps_the_bytes() {
        for i in 0..=255u8 {
            assert_eq!(Encoding::Srgb.byte(i), i);
        }
    }

    #[test]
    fn unorm_format_stores_linear_light() {
        let t = Encoding::Linear.table();
        assert_eq!(t[0], 0);
        assert_eq!(t[255], 255);
        // code 128 (50 % grey) is 21.6 % linear light
        assert_eq!(t[128], 55);
        assert!(t.windows(2).all(|w| w[0] <= w[1]), "monotonic");
        // what the compositor does with it (linear -> sRGB) gives the authored code back (within quantisation)
        for code in [60usize, 100, 137, 200, 250] {
            let back = linear_to_srgb(f32::from(t[code]) / 255.0) * 255.0;
            assert!((back - code as f32).abs() <= 4.0, "{code} -> {back}");
        }
    }

    #[test]
    fn format_choice() {
        let offered = [
            VK_FORMAT_R8G8B8A8_UNORM,
            VK_FORMAT_B8G8R8A8_SRGB,
            VK_FORMAT_R8G8B8A8_SRGB,
            64,
        ];
        assert_eq!(
            choose_format(&offered, false),
            Some(VK_FORMAT_R8G8B8A8_SRGB)
        );
        assert_eq!(
            choose_format(&offered, true),
            Some(VK_FORMAT_R8G8B8A8_UNORM)
        );
        assert_eq!(
            choose_format(&[VK_FORMAT_B8G8R8A8_UNORM], false),
            Some(VK_FORMAT_B8G8R8A8_UNORM)
        );
        // only BGRA sRGB: still sRGB
        assert_eq!(
            choose_format(&[VK_FORMAT_B8G8R8A8_UNORM, VK_FORMAT_B8G8R8A8_SRGB], false),
            Some(VK_FORMAT_B8G8R8A8_SRGB)
        );
        assert_eq!(choose_format(&[64, 99], false), None);
        assert_eq!(encoding_of(VK_FORMAT_B8G8R8A8_SRGB), Encoding::Srgb);
        assert_eq!(encoding_of(VK_FORMAT_R8G8B8A8_UNORM), Encoding::Linear);
        assert!(is_bgra(VK_FORMAT_B8G8R8A8_UNORM) && !is_bgra(VK_FORMAT_R8G8B8A8_SRGB));
        assert_eq!(format_name(43), "R8G8B8A8_SRGB");
        assert_eq!(format_name(1), "VkFormat 1");
    }

    #[test]
    fn upload_bytes_per_format() {
        // one pixel with an sRGB code 128 colour and half alpha
        let src = [200u8, 128, 30, 100];
        let mut out = [0u8; 4];
        // sRGB RGBA: unchanged
        encode_for_format(&src, &mut out, VK_FORMAT_R8G8B8A8_SRGB);
        assert_eq!(out, src);
        // sRGB BGRA: only the channel order changes
        encode_for_format(&src, &mut out, VK_FORMAT_B8G8R8A8_SRGB);
        assert_eq!(out, [30, 128, 200, 100]);
        // UNORM RGBA: colour is linearised, alpha untouched
        encode_for_format(&src, &mut out, VK_FORMAT_R8G8B8A8_UNORM);
        assert_eq!(
            out,
            [
                Encoding::Linear.byte(200),
                55,
                Encoding::Linear.byte(30),
                100
            ]
        );
        assert!(out[0] < src[0] && out[2] < src[2]);
        // UNORM BGRA: linearised and swapped
        encode_for_format(&src, &mut out, VK_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(
            out,
            [
                Encoding::Linear.byte(30),
                55,
                Encoding::Linear.byte(200),
                100
            ]
        );
    }

    #[test]
    #[should_panic(expected = "differ in size")]
    fn encode_checks_sizes() {
        encode_into(&[0; 8], &mut [0; 4], Encoding::Srgb, false);
    }
}
