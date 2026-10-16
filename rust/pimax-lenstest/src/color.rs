//! Colours. Patterns work in "panel code" space: a value of 1.0 is the brightest the panel shows, 0.5 is what the
//! panel shows for code 128. The upload step turns codes into the bytes of the swapchain format so that the code
//! reaches the panel unchanged (see [`Encoding`]).

pub type Rgb = [f32; 3];

pub const BLACK: Rgb = [0.0, 0.0, 0.0];
pub const WHITE: Rgb = [1.0, 1.0, 1.0];
pub const RED: Rgb = [1.0, 0.0, 0.0];
pub const GREEN: Rgb = [0.0, 1.0, 0.0];
pub const BLUE: Rgb = [0.0, 0.0, 1.0];
pub const YELLOW: Rgb = [1.0, 1.0, 0.0];
pub const CYAN: Rgb = [0.0, 1.0, 1.0];
pub const MAGENTA: Rgb = [1.0, 0.0, 1.0];
pub const ORANGE: Rgb = [1.0, 0.5, 0.0];

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

/// The panel code that gives `linear` (0..1) of the full-white luminance, assuming the panel follows the sRGB curve.
/// `level(0.25)` is the code for "25% brightness".
pub fn level(linear: f32) -> f32 {
    linear_to_srgb(linear.clamp(0.0, 1.0))
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub fn gray(v: f32) -> Rgb {
    [v, v, v]
}

pub fn to_byte(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// How the compositor treats the bytes of the swapchain format.
///
/// Monado's compositor writes `from_linear_to_srgb(sample)` into the display target. An `*_SRGB` swapchain is
/// sampled through an sRGB view (decoded to linear, then encoded again): the bytes reach the panel unchanged
/// ([`Encoding::Srgb`]). A `*_UNORM` swapchain is sampled as linear, so the app must store the decoded value to get
/// the same code on the panel ([`Encoding::Linear`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Srgb,
    Linear,
}

impl Encoding {
    /// Byte to store for the panel code `code` (0..1).
    pub fn byte(self, code: f32) -> u8 {
        match self {
            Encoding::Srgb => to_byte(code),
            Encoding::Linear => to_byte(srgb_to_linear(code.clamp(0.0, 1.0))),
        }
    }

    /// 256-entry table: code byte -> stored byte.
    pub fn table(self) -> [u8; 256] {
        let mut t = [0u8; 256];
        for (i, e) in t.iter_mut().enumerate() {
            *e = self.byte(i as f32 / 255.0);
        }
        t
    }
}

pub const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;
pub const VK_FORMAT_B8G8R8A8_UNORM: u32 = 44;
pub const VK_FORMAT_B8G8R8A8_SRGB: u32 = 50;

/// Pick a swapchain format from the ones the runtime offers: sRGB first (bytes reach the panel unchanged), then
/// UNORM; `prefer_unorm` swaps the order. RGBA before BGRA.
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
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn quarter_brightness_is_code_137() {
        let l = level(0.25);
        assert!((l - 0.537).abs() < 1e-3, "{l}");
        assert_eq!(to_byte(l), 137);
        assert_eq!(to_byte(level(1.0)), 255);
        assert_eq!(to_byte(level(0.0)), 0);
        assert_eq!(to_byte(level(7.0)), 255); // clamped
                                              // luminance of code 137 is 25% of white
        assert!((srgb_to_linear(137.0 / 255.0) - 0.25).abs() < 0.003);
    }

    #[test]
    fn encodings() {
        for i in 0..=255u32 {
            assert_eq!(Encoding::Srgb.byte(i as f32 / 255.0), i as u8);
        }
        // UNORM: store the linear value, so that the compositor's linear -> sRGB encode gives the code back
        let t = Encoding::Linear.table();
        assert_eq!(t[0], 0);
        assert_eq!(t[255], 255);
        for code in [30usize, 100, 137, 200, 250] {
            let back = to_byte(linear_to_srgb(t[code] as f32 / 255.0)) as i32;
            assert!((back - code as i32).abs() <= 4, "{code} -> {back}");
        }
        assert!(t.windows(2).all(|w| w[0] <= w[1]));
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
        assert_eq!(choose_format(&[64, 99], false), None);
        assert_eq!(encoding_of(VK_FORMAT_B8G8R8A8_SRGB), Encoding::Srgb);
        assert_eq!(encoding_of(VK_FORMAT_R8G8B8A8_UNORM), Encoding::Linear);
        assert!(is_bgra(VK_FORMAT_B8G8R8A8_UNORM) && !is_bgra(VK_FORMAT_R8G8B8A8_SRGB));
        assert_eq!(format_name(43), "R8G8B8A8_SRGB");
        assert_eq!(format_name(1), "VkFormat 1");
    }

    #[test]
    fn mix_and_gray() {
        assert_eq!(mix(BLACK, WHITE, 0.25), gray(0.25));
        assert_eq!(mix(RED, BLUE, 2.0), BLUE);
        assert_eq!(mix(RED, BLUE, -1.0), RED);
    }
}
