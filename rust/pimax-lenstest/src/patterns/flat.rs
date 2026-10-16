//! Full-field uniform colour for comparing the colour balance of the two eyes. Seven levels (stick left/right):
//! white 255, grey 192, 128, 64, then red, green, blue at 255. Only a small label in one corner (the text toggle
//! hides it); everything else is the uniform colour.

use super::{text_scale, Params, LABEL_BG};
use crate::color::{gray, Rgb, BLUE, GREEN, RED, WHITE};
use crate::image::Image;

pub const STEPS: usize = 7;

pub fn step_name(step: usize) -> &'static str {
    match step {
        0 => "WHITE 255",
        1 => "GREY 192",
        2 => "GREY 128",
        3 => "GREY 64",
        4 => "RED 255",
        5 => "GREEN 255",
        _ => "BLUE 255",
    }
}

/// Panel code colour of a step.
pub fn color(step: usize) -> Rgb {
    match step {
        0 => WHITE,
        1 => gray(192.0 / 255.0),
        2 => gray(128.0 / 255.0),
        3 => gray(64.0 / 255.0),
        4 => RED,
        5 => GREEN,
        _ => BLUE,
    }
}

/// The level name in the lower left, inside the lens-visible area (about 27 deg left, 29 deg down of the axis).
pub fn draw_label(img: &mut Image, p: &Params) {
    let scale = text_scale(p.h);
    let (x, y) = p.to_pixel(-0.5, -0.55);
    img.label(
        step_name(p.step),
        x as i32,
        y as i32,
        scale,
        gray(0.5),
        LABEL_BG,
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::super::{render, Pattern};

    #[test]
    fn levels_are_uniform_and_in_order() {
        let mut p = params(200, 180);
        let want = [
            [255, 255, 255],
            [192, 192, 192],
            [128, 128, 128],
            [64, 64, 64],
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
        ];
        for (i, w) in want.iter().enumerate() {
            p.step = i;
            let img = render(Pattern::Flat, &p, None, 2);
            assert_eq!(img.count(|c| c == *w), 200 * 180, "step {i}");
        }
        assert_eq!(Pattern::Flat.steps(&p), 7);
    }

    #[test]
    fn label_only_in_a_corner_and_hideable() {
        let mut p = params(1000, 900);
        p.step = 4;
        let hud = vec!["X".to_string()];
        let with = render(Pattern::Flat, &p, Some(&hud), 2);
        let without = render(Pattern::Flat, &p, None, 2);
        let n = without.count(|c| c == [255, 0, 0]);
        assert_eq!(n, 1000 * 900);
        let diff = with.count(|c| c != [255, 0, 0]);
        assert!(diff > 20 && diff < 1000 * 900 / 50, "{diff}");
        // all the changed pixels are in the lower-left quarter
        for y in 0..900 {
            for x in 0..1000 {
                if with.get(x, y) != [255, 0, 0] {
                    assert!(x < 500 && y > 450, "{x},{y}");
                }
            }
        }
    }
}
