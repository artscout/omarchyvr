//! The `gaze` pattern as a still picture (previews, `simulate`, the list): the head-locked dots of the axis check and
//! of the 17-point accuracy grid at 2 m, as the eye described by `Params::view` sees them. The live test (shrinking
//! dot, gaze cursor, text, statistics) is drawn by [`crate::gazedraw`] and driven by [`crate::gaze::GazeTest`].

use super::Params;
use crate::color::Rgb;
use crate::gaze::{axis_targets, grid_targets, Mark, MarkKind};
use crate::gazedraw::{shade_marks, EyePose};

/// The marks of the still picture.
pub fn marks() -> Vec<Mark> {
    axis_targets()
        .into_iter()
        .chain(grid_targets(17))
        .map(|t| Mark {
            pos: t.point(),
            kind: MarkKind::Disc,
            radius_deg: 0.5,
            color: [1.0, 1.0, 1.0],
        })
        .collect()
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let pose = EyePose {
        rot: p.view.rot,
        pos: p.view.pos,
    };
    shade_marks(&marks(), &pose, &p.fov, p.w, p.h, x, y)
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::super::{render, Pattern};

    #[test]
    fn preview_shows_the_dots_and_nothing_else() {
        let p = params(300, 270);
        let img = render(Pattern::Gaze, &p, None, 2);
        let lit = img.count(|c| c != [0, 0, 0]);
        assert!(lit > 50 && lit < 3000, "{lit}");
        let (px, py) = p.fov.tangent_to_pixel(p.w, p.h, 0.016, 0.0);
        assert!(
            img.get(px as u32, py as u32)[0] > 200,
            "centre dot at the left eye's tangent 0.016"
        );
        assert_eq!(Pattern::Gaze.steps(&p), 1);
    }
}
