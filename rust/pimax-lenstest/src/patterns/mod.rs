//! The test patterns. Every pattern is a pure function `shade(params, x, y) -> colour` of the pixel position and the
//! per-eye projection (so the pattern is correct for whatever FOV the runtime reports), plus a list of text labels.
//! `render` assembles the image: rows in parallel, labels, optional on-screen instructions.

pub mod align;
pub mod border;
pub mod channels;
pub mod corners;
pub mod flat;
pub mod gaze;
pub mod grid;
pub mod rings;
pub mod square;
pub mod swim;

use crate::color::{self, Rgb, BLACK};
use crate::geometry::{dir_to_tangent, mat_vec, transpose, Fov, Mat3};
use crate::image::Image;
use crate::tune::Warp;

/// Direction in which the single square moves away from the centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Right,
    Left,
    Up,
    Down,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Right, Dir::Left, Dir::Up, Dir::Down];

    pub fn next(self) -> Dir {
        match self {
            Dir::Right => Dir::Left,
            Dir::Left => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Right,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Dir::Right => "right",
            Dir::Left => "left",
            Dir::Up => "up",
            Dir::Down => "down",
        }
    }

    pub fn from_name(s: &str) -> Option<Dir> {
        Dir::ALL
            .into_iter()
            .find(|d| d.name() == s.to_ascii_lowercase())
    }
}

/// Everything a pattern needs to know about the eye image it draws.
#[derive(Clone, Debug)]
pub struct Params {
    pub w: u32,
    pub h: u32,
    pub fov: Fov,
    /// 0 = left eye, 1 = right eye.
    pub eye: usize,
    /// Sub-step of the pattern (see [`Pattern::steps`]).
    pub step: usize,
    /// Angular size of the moving square in degrees.
    pub square_deg: f32,
    pub square_dir: Dir,
    /// Align pattern: shift of this eye's target in degrees (right, up).
    pub align_shift_deg: [f32; 2],
    /// Eye space -> head space rotation. `Some`: the grid-like patterns are centred on the head-forward direction
    /// (the two eyes' crosses fuse when the displays are canted); `None`: centred on this eye's optical axis.
    pub head: Option<Mat3>,
    /// Residual radial warp of the image (live distortion tuning), identity when off.
    pub warp: Warp,
    /// Eye pose in the world for the `swim` pattern.
    pub view: swim::SwimView,
}

impl Params {
    pub fn new(w: u32, h: u32, fov: Fov, eye: usize) -> Params {
        Params {
            w,
            h,
            fov,
            eye,
            step: 0,
            square_deg: square::DEFAULT_SIZE_DEG,
            square_dir: Dir::Right,
            align_shift_deg: [0.0; 2],
            head: None,
            warp: Warp::IDENTITY,
            view: swim::SwimView::default_for_eye(eye),
        }
    }

    /// Pattern-space tangent (right, up) of the centre of pixel (x, y): the pixel's ray, through the residual warp,
    /// seen from the head when `head` is set.
    pub fn tangent(&self, x: u32, y: u32) -> (f32, f32) {
        let t = self
            .warp
            .apply(self.fov.pixel_to_tangent(self.w, self.h, x, y));
        match &self.head {
            None => t,
            Some(m) => dir_to_tangent(mat_vec(m, [t.0, t.1, -1.0])),
        }
    }

    /// Pixel position (continuous) of the pattern-space tangent point; inverse of [`Params::tangent`].
    pub fn to_pixel(&self, tx: f32, ty: f32) -> (f32, f32) {
        let t = match &self.head {
            None => (tx, ty),
            Some(m) => dir_to_tangent(mat_vec(&transpose(m), [tx, ty, -1.0])),
        };
        let t = self.warp.invert(t);
        self.fov.tangent_to_pixel(self.w, self.h, t.0, t.1)
    }

    /// Pixel position of the pattern centre (pattern tangent 0, 0): head-forward in head mode, else the optical axis.
    pub fn principal(&self) -> (f32, f32) {
        self.to_pixel(0.0, 0.0)
    }

    /// Faint marker of this eye's optical axis, drawn over head-centred patterns.
    fn axis_marker(&self, x: u32, y: u32, c: Rgb) -> Rgb {
        if self.head.is_none() {
            return c;
        }
        let (tx, ty) = self.fov.pixel_to_tangent(self.w, self.h, x, y);
        let pix = self.fov.tangent_per_pixel(self.w);
        let arm = 0.05;
        let a = if ty.abs() < arm {
            cover(tx, pix * 0.8, pix)
        } else {
            0.0
        }
        .max(if tx.abs() < arm {
            cover(ty, pix * 0.8, pix)
        } else {
            0.0
        });
        color::mix(c, [0.55, 0.25, 0.25], 0.55 * a)
    }
}

/// A text label placed in the image (top-left of the text at x, y).
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub scale: u32,
    pub fg: Rgb,
    pub bg: Rgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    Grid,
    Rings,
    Channels,
    FovEdge,
    Square100,
    Square25,
    Corners,
    Align,
    Swim,
    Flat,
    Gaze,
}

impl Pattern {
    /// The patterns of the default cycle: everything except `gaze` (it starts the eye chip and needs a service with
    /// `PIMAX_EYE=1`, so it is only shown when asked for with `--patterns gaze`).
    pub const DEFAULT_CYCLE: [Pattern; 10] = [
        Pattern::Grid,
        Pattern::Rings,
        Pattern::Channels,
        Pattern::FovEdge,
        Pattern::Square100,
        Pattern::Square25,
        Pattern::Corners,
        Pattern::Align,
        Pattern::Swim,
        Pattern::Flat,
    ];

    pub const ALL: [Pattern; 11] = [
        Pattern::Grid,
        Pattern::Rings,
        Pattern::Channels,
        Pattern::FovEdge,
        Pattern::Square100,
        Pattern::Square25,
        Pattern::Corners,
        Pattern::Align,
        Pattern::Swim,
        Pattern::Flat,
        Pattern::Gaze,
    ];

    /// Short machine name (command line, file names).
    pub fn name(self) -> &'static str {
        match self {
            Pattern::Grid => "grid",
            Pattern::Rings => "rings",
            Pattern::Channels => "channels",
            Pattern::FovEdge => "fov-edge",
            Pattern::Square100 => "square100",
            Pattern::Square25 => "square25",
            Pattern::Corners => "corners",
            Pattern::Align => "align",
            Pattern::Swim => "swim",
            Pattern::Flat => "flat",
            Pattern::Gaze => "gaze",
        }
    }

    pub fn from_name(s: &str) -> Option<Pattern> {
        let s = s.to_ascii_lowercase();
        Pattern::ALL.into_iter().find(|p| p.name() == s)
    }

    pub fn title(self) -> &'static str {
        match self {
            Pattern::Grid => "STRAIGHT-LINE GRID",
            Pattern::Rings => "ANGLE RINGS 10-50 DEG",
            Pattern::Channels => "PER-CHANNEL GRIDS",
            Pattern::FovEdge => "FOV EDGE AND BORDER",
            Pattern::Square100 => "MOVING SQUARE 100%",
            Pattern::Square25 => "MOVING SQUARE 25%",
            Pattern::Corners => "CORNER FRAMES",
            Pattern::Align => "STEREO ALIGNMENT",
            Pattern::Swim => "PUPIL SWIM (WORLD-LOCKED)",
            Pattern::Flat => "FLAT COLOUR",
            Pattern::Gaze => "EYE TRACKING ACCURACY",
        }
    }

    /// What this pattern is for, one sentence.
    pub fn purpose(self) -> &'static str {
        match self {
            Pattern::Grid => "tests the distortion model: straight lines of the scene must look straight",
            Pattern::Rings => "tests the angle-to-radius curve: circles at known angles from the optical axis",
            Pattern::Channels => "tests chromatic aberration: one colour channel at a time against the others",
            Pattern::FovEdge => "tests the edge of the rendered image: clamping, streaks, visible shape of the panel",
            Pattern::Square100 => "tells glare from distortion: a single bright square on black, full brightness",
            Pattern::Square25 => "tells glare from distortion: the same square at a quarter of the light",
            Pattern::Corners => "tests orientation and the panel corners: coloured brackets and checker patches",
            Pattern::Align => "measures the per-eye lens centre: shift the two eye targets until they fuse, read off lens.conf center",
            Pattern::Swim => "tests pupil swim: a world-locked room of straight lines and poles; turn your head and watch the lines",
            Pattern::Flat => "compares the colour balance of the two eyes: full-field uniform colour or grey level",
            Pattern::Gaze => "measures the eye tracker: head-locked dots at 2 m, axis-sign check, accuracy grid, live gaze cursor, host-side correction fit (needs the service with PIMAX_EYE=1)",
        }
    }

    /// What to look for, as the user reads it in the headset or in the terminal (one entry per line).
    pub fn instructions(self) -> &'static [&'static str] {
        match self {
            Pattern::Grid => &[
                "Every grid line must be STRAIGHT, also near the edges and corners.",
                "Lines bowing OUT (barrel) or IN (pincushion) mean the distortion model is wrong there.",
                "The red cross is the optical axis: it must be straight ahead of your pupil (close one eye at a time).",
                "Squares should look square near the centre and not shear towards the corners.",
            ],
            Pattern::Rings => &[
                "Circles at 10/20/30/40/50 deg from the optical axis (labelled), thin grey ones every 5 deg.",
                "They must look ROUND and evenly spaced in angle, not oval, not bunched up near the edge.",
                "Compare with a real angle: look 30 deg to the side, the 30 ring should be where you then look.",
                "All rings too small or large: base/scale. Only the outer ones: k2/k3. Cross off-centre: centre.",
            ],
            Pattern::Channels => &[
                "ALL step: thin white lines must have NO coloured fringes, especially at the 40-50 deg rings.",
                "Red outside / blue inside (or the reverse) on a line means ca_r/ca_b (or per-channel k) are off.",
                "R, G, B steps show one colour only: switch between them and note where the outer ring sits.",
                "If red or blue sits farther out than green on the 40 deg ring, say by how many mm (checklist).",
            ],
            Pattern::FovEdge => &[
                "FRAMES: the outermost frame (red left, green right, blue top, yellow bottom) must be complete.",
                "Anything bright OUTSIDE it (streaks, repeats, smear to the panel edge) = clamping or wrap bug.",
                "The white frames are 2, 5 and 10 deg inside the edge: which ones can you still see?",
                "WHITE and GRAY fill: the bright shape is the visible area; look for halos or glow at its edge.",
            ],
            Pattern::Square100 => &[
                "One 5 deg white square on black, moved from the centre to the edge (stick left/right).",
                "A halo or ghost that stays at the square's position and scales with brightness is lens glare.",
                "A copy at a FIXED offset from the square (not following its brightness) is software sampling.",
                "A square that changes SHAPE towards the edge is distortion. Compare with the 25% pattern.",
            ],
            Pattern::Square25 => &[
                "Same square at a quarter of the light. At every position, compare with the 100% pattern.",
                "Ghost much weaker or gone at 25% = optical glare (hardware, not fixable in the driver).",
                "Ghost equally strong at 25% = not glare: report its offset and direction.",
                "Note the position where the ghost starts (deg from centre) and which side (nose or temple).",
            ],
            Pattern::Corners => &[
                "Brackets: top-left red, top-right green, bottom-left blue, bottom-right yellow in EACH eye.",
                "A bracket in the wrong corner means the panel rotation or mirroring is wrong.",
                "Checker patches fill the corners: sharp, complete, not smeared, mirrored or cut off?",
                "Which brackets are fully visible and which are cut by the lens? Write it in the checklist.",
            ],
            Pattern::Align => &[
                "Left eye: red line, right eye: green, each on its optical axis. Look with both eyes: you want ONE line.",
                "Keys [ ] shift the eyes 0.05 deg against each other (stick L/R); { } 0.25 deg; I K vertical; R reset.",
                "When the lines fuse, read the [left]/[right] center values (screen and terminal) and quit with Q.",
                "The result is printed as a block to paste into a variant lens.conf; lens.conf itself is not changed.",
            ],
            Pattern::Swim => &[
                "A room 4 m wide with a 25 cm line grid on all six faces (bright every metre) and poles at 0, 20, 40, 60 deg.",
                "Look at the centre and turn your head slowly left/right and up/down: lines must stay straight and still.",
                "Lines that bend or bulge as they move to the edge: barrel (bulge out) or pincushion (bend in) residual.",
                "Keys ; ' (0.01) and : \" (0.05) change dk1, - = (0.5%) and _ + (2%) the scale; R resets, C re-centres.",
                "Keys o l (0.01), O L (0.05): dk2 = magnification of the mid zone (24 deg); the edge stays.",
                "On Q the equivalent lens.conf scale and k_g/k_r/k_b are printed; restart Monado to use them.",
            ],
            Pattern::Flat => &[
                "The whole field is one colour: WHITE 255, GREY 192, 128, 64, then RED, GREEN, BLUE at 255 (stick L/R or , .).",
                "Close one eye at a time and compare: brightness, tint, uniformity (vignetting, colour cast).",
                "The level name is a small label in one corner; the text key (grip or H) hides it.",
            ],
            Pattern::Gaze => &[
                "Needs the service with PIMAX_EYE=1 (analysis/tools/eye_accuracy.sh starts it). Keep your head still.",
                "Axis check first: 5 dots (centre, 15 deg right/left/up/down); a mirrored axis names its PIMAX_EYE_FLIP_*.",
                "Then the accuracy grid, 17 dots up to 35 deg out: look at each dot (0.6 s settle, then 1 s measured).",
                "Result: error mean/median/95%/max, precision, valid %, centre/mid/edge, saved as JSON (path is printed).",
                "Then a live ring shows where the tracker thinks you look; the green ring is the fitted correction.",
                "Keys: SPACE/A next, R restart, Y accept, write ~/.config/omarchyvr/eye-correction.toml (backup kept), Q quit.",
            ],
        }
    }

    /// Two short lines for the on-screen text in the headset.
    pub fn hud_lines(self) -> [&'static str; 2] {
        match self {
            Pattern::Grid => [
                "ARE ALL LINES STRAIGHT, ALSO AT THE EDGES?",
                "RED CROSS STRAIGHT AHEAD OF THE PUPIL?",
            ],
            Pattern::Rings => [
                "ROUND AND EVENLY SPACED IN ANGLE?",
                "10 20 30 40 50 DEG FROM THE OPTICAL AXIS",
            ],
            Pattern::Channels => [
                "COLOURED FRINGES ON THE LINES?",
                "OUTER RING: RED/BLUE FARTHER OUT THAN GREEN?",
            ],
            Pattern::FovEdge => [
                "IS THE OUTER FRAME COMPLETE? ANYTHING BEYOND IT?",
                "HALO OR STREAKS AT THE EDGE?",
            ],
            Pattern::Square100 => [
                "GHOST OR HALO NEXT TO THE SQUARE?",
                "SAME PLACE AT 25%? STICK L/R MOVES THE SQUARE",
            ],
            Pattern::Square25 => [
                "GHOST WEAKER OR GONE AT 25%?",
                "STICK L/R MOVES THE SQUARE, UP/DOWN CHANGES SIDE",
            ],
            Pattern::Corners => [
                "TL RED TR GREEN BL BLUE BR YELLOW IN EACH EYE?",
                "ALL FOUR CORNERS VISIBLE AND SHARP?",
            ],
            Pattern::Align => [
                "RED LINE LEFT EYE, GREEN RIGHT EYE: MAKE THEM ONE",
                "SHIFT THE EYES AGAINST EACH OTHER UNTIL THEY FUSE",
            ],
            Pattern::Swim => [
                "LOOK AT THE CENTRE, TURN YOUR HEAD SLOWLY LEFT/RIGHT:",
                "LINES MUST STAY STRAIGHT AND STILL. BENDING AT THE EDGE: BARREL/PINCUSHION",
            ],
            Pattern::Flat => ["", ""],
            Pattern::Gaze => ["", ""],
        }
    }

    /// Number of sub-steps (stick left/right) of the pattern for these parameters.
    pub fn steps(self, p: &Params) -> usize {
        match self {
            Pattern::Channels => channels::STEPS,
            Pattern::Flat => flat::STEPS,
            Pattern::FovEdge => border::STEPS,
            Pattern::Square100 | Pattern::Square25 => square::positions(p).len(),
            _ => 1,
        }
    }

    pub fn step_name(self, p: &Params, step: usize) -> String {
        match self {
            Pattern::Channels => channels::step_name(step).to_string(),
            Pattern::Flat => flat::step_name(step).to_string(),
            Pattern::FovEdge => border::step_name(step).to_string(),
            Pattern::Square100 | Pattern::Square25 => square::step_name(p, step),
            _ => String::new(),
        }
    }

    /// True for patterns that measure glare: the on-screen text would itself glare, so it fades out quickly.
    pub fn hides_hud(self) -> bool {
        matches!(
            self,
            Pattern::Square100 | Pattern::Square25 | Pattern::FovEdge
        )
    }

    pub fn shade(self, p: &Params, x: u32, y: u32) -> Rgb {
        let c = self.shade_pattern(p, x, y);
        match self {
            Pattern::Grid | Pattern::Rings | Pattern::Channels | Pattern::Align => {
                p.axis_marker(x, y, c)
            }
            _ => c,
        }
    }

    fn shade_pattern(self, p: &Params, x: u32, y: u32) -> Rgb {
        match self {
            Pattern::Grid => grid::shade(p, x, y),
            Pattern::Rings => rings::shade(p, x, y),
            Pattern::Channels => channels::shade(p, x, y),
            Pattern::Swim => swim::shade(p, x, y),
            Pattern::Flat => flat::color(p.step),
            Pattern::Gaze => gaze::shade(p, x, y),
            Pattern::FovEdge => border::shade(p, x, y),
            Pattern::Square100 => square::shade(p, x, y, 1.0),
            Pattern::Square25 => square::shade(p, x, y, 0.25),
            Pattern::Corners => corners::shade(p, x, y),
            Pattern::Align => align::shade(p, x, y),
        }
    }

    pub fn labels(self, p: &Params) -> Vec<Label> {
        match self {
            Pattern::Grid => grid::labels(p),
            Pattern::Rings => rings::labels(p),
            Pattern::Channels => channels::labels(p),
            Pattern::FovEdge => border::labels(p),
            Pattern::Square100 | Pattern::Square25 => Vec::new(),
            Pattern::Corners => corners::labels(p),
            Pattern::Align | Pattern::Swim | Pattern::Flat | Pattern::Gaze => Vec::new(),
        }
    }
}

/// Anti-aliased coverage of a line of half-width `hw` at distance `d` from its centre line (all in the same units,
/// `pix` = size of one pixel).
pub fn cover(d: f32, hw: f32, pix: f32) -> f32 {
    ((hw - d.abs()) / pix + 0.5).clamp(0.0, 1.0)
}

/// Background of the grid-like patterns: a faint tint per eye so the eyes can be told apart.
pub fn eye_background(eye: usize) -> Rgb {
    if eye == 0 {
        [0.02, 0.04, 0.09]
    } else {
        [0.09, 0.03, 0.03]
    }
}

/// Text scale (pixels per font pixel) that gives labels of about one degree for an image of height `h`.
pub fn text_scale(h: u32) -> u32 {
    (h / 520).max(1)
}

/// The label backdrop: nearly black, so labels do not add a glow.
pub const LABEL_BG: Rgb = [0.02, 0.02, 0.02];

/// Render the whole eye image: the pattern, its labels, and the on-screen instructions if given.
pub fn render(pattern: Pattern, p: &Params, hud: Option<&[String]>, threads: usize) -> Image {
    let mut img = Image::render(p.w, p.h, threads, &|x, y| pattern.shade(p, x, y));
    for l in pattern.labels(p) {
        img.label(&l.text, l.x, l.y, l.scale, l.fg, l.bg);
    }
    if let Some(lines) = hud {
        if pattern == Pattern::Flat {
            flat::draw_label(&mut img, p);
        } else {
            draw_hud_at(&mut img, &p.fov, p.head.as_ref(), lines);
        }
    }
    img
}

/// A black image (the eye that is switched off).
pub fn blank(w: u32, h: u32) -> Image {
    Image::new(w, h, BLACK)
}

/// Break `text` into lines of at most `max` characters at spaces.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > max {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Where a head-locked text block is anchored in an eye image: (x of the block centre, y of the first row's
/// reference line), both in pixels. With `head` (eye space -> head space rotation, i.e. canted displays) it is the
/// head-forward direction's pixel, so the two eyes' copies of the text land on the same direction and fuse; without,
/// the optical axis. This is the fallback when the text cannot be a compositor quad layer.
pub fn hud_origin(w: u32, h: u32, fov: &Fov, head: Option<&Mat3>) -> (f32, f32) {
    let mut p = Params::new(w, h, *fov, 0);
    p.head = head.copied();
    (p.to_pixel(0.0, 0.0).0, p.to_pixel(0.0, -0.45).1)
}

/// Layout of the HUD block: text scale, first row (below the anchor), row pitch.
fn hud_layout(h: u32, origin: (f32, f32), n_lines: usize) -> (u32, i32, i32) {
    let scale = text_scale(h);
    let line_h = (7 + 3) * scale as i32;
    let block_h = line_h * n_lines as i32;
    let y0 = (origin.1 as i32)
        .min(h as i32 - block_h - 4 * scale as i32)
        .max(0);
    (scale, y0, line_h)
}

/// Left edge of a HUD line of text width `tw`.
fn hud_x(w: u32, origin: (f32, f32), tw: u32, scale: u32) -> i32 {
    (origin.0 as i32 - tw as i32 / 2).clamp(
        scale as i32,
        (w as i32 - tw as i32 - scale as i32).max(scale as i32),
    )
}

pub const HUD_FG: f32 = 0.72;
pub const HUD_BG: Rgb = [0.01, 0.01, 0.01];

/// Draw the instruction lines as a block centred on the optical axis, below it, on near-black backdrops. The text
/// colour is a calm light grey: it must not be the brightest thing in the image.
pub fn draw_hud(img: &mut Image, fov: &Fov, lines: &[String]) {
    draw_hud_at(img, fov, None, lines);
}

/// [`draw_hud`] anchored per [`hud_origin`] (`head` = the eye-to-head rotation of canted displays).
pub fn draw_hud_at(img: &mut Image, fov: &Fov, head: Option<&Mat3>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let origin = hud_origin(img.w, img.h, fov, head);
    let (scale, y0, line_h) = hud_layout(img.h, origin, lines.len());
    for (i, line) in lines.iter().enumerate() {
        let (tw, _) = pimax_lens::font::text_size(line, scale);
        let x = hud_x(img.w, origin, tw, scale);
        img.label(
            line,
            x,
            y0 + i as i32 * line_h,
            scale,
            color::gray(HUD_FG),
            HUD_BG,
        );
    }
}

/// The HUD block as a strip image (full image width) and the image row where the strip starts, for the GPU path to
/// composite: pixels that are exactly black are transparent, every drawn pixel (backdrop included) is not. Same
/// pixels as [`draw_hud`] puts into the image.
pub fn hud_strip(w: u32, h: u32, fov: &Fov, lines: &[String]) -> Option<(i32, Image)> {
    hud_strip_at(w, h, fov, None, lines)
}

/// [`hud_strip`] anchored per [`hud_origin`].
pub fn hud_strip_at(
    w: u32,
    h: u32,
    fov: &Fov,
    head: Option<&Mat3>,
    lines: &[String],
) -> Option<(i32, Image)> {
    if lines.is_empty() {
        return None;
    }
    let origin = hud_origin(w, h, fov, head);
    let (scale, y0, line_h) = hud_layout(h, origin, lines.len());
    let top = (y0 - scale as i32).max(0);
    let bottom = (y0 + line_h * lines.len() as i32 + scale as i32).min(h as i32);
    if bottom <= top {
        return None;
    }
    let mut img = Image::new(w, (bottom - top) as u32, BLACK);
    for (i, line) in lines.iter().enumerate() {
        let (tw, _) = pimax_lens::font::text_size(line, scale);
        let x = hud_x(w, origin, tw, scale);
        img.label(
            line,
            x,
            y0 - top + i as i32 * line_h,
            scale,
            color::gray(HUD_FG),
            HUD_BG,
        );
    }
    Some((top, img))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn params(w: u32, h: u32) -> Params {
        Params::new(w, h, Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5), 0)
    }

    #[test]
    fn names_roundtrip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in Pattern::ALL {
            assert_eq!(Pattern::from_name(p.name()), Some(p));
            assert_eq!(Pattern::from_name(&p.name().to_uppercase()), Some(p));
            assert!(seen.insert(p.name()));
            assert!(!p.title().is_empty() && !p.purpose().is_empty());
            assert!(p.instructions().len() >= 3, "{p:?}");
            for l in p.instructions() {
                assert!(l.len() < 110, "{l}");
            }
            for l in p.hud_lines() {
                assert!(
                    l.chars().all(|c| c.is_ascii_uppercase()
                        || c.is_ascii_digit()
                        || " /,.:?%-".contains(c)),
                    "{l}"
                );
            }
        }
        assert_eq!(Pattern::from_name("nope"), None);
        assert_eq!(Dir::from_name("UP"), Some(Dir::Up));
        assert_eq!(Dir::Right.next().next().next().next(), Dir::Right);
    }

    #[test]
    fn steps_and_step_names() {
        let p = params(100, 100);
        assert_eq!(Pattern::Grid.steps(&p), 1);
        assert_eq!(Pattern::Channels.steps(&p), 4);
        assert_eq!(Pattern::FovEdge.steps(&p), 3);
        assert_eq!(Pattern::Square100.steps(&p), Pattern::Square25.steps(&p));
        assert!(Pattern::Square100.steps(&p) >= 5);
        assert_eq!(Pattern::Grid.step_name(&p, 0), "");
        for pat in Pattern::ALL {
            for s in 0..pat.steps(&p) {
                let _ = pat.step_name(&p, s);
            }
        }
    }

    #[test]
    fn cover_is_a_clamped_ramp() {
        assert_eq!(cover(0.0, 1.0, 1.0), 1.0);
        assert_eq!(cover(2.0, 1.0, 1.0), 0.0);
        assert!((cover(1.0, 1.0, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(cover(-0.2, 1.0, 1.0), cover(0.2, 1.0, 1.0));
    }

    #[test]
    fn wrap_breaks_at_spaces() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("aaaaaaaaaa b", 4), vec!["aaaaaaaaaa", "b"]);
        assert!(wrap("", 10).is_empty());
        assert!(wrap("a b c d e f g", 5).iter().all(|l| l.len() <= 5));
    }

    #[test]
    fn fallback_text_is_shifted_by_the_head_forward_offset() {
        let p = params(400, 360);
        let (px, _) = p.fov.principal_point(400, 360);
        // no head rotation: the optical axis, as before
        let none = hud_origin(400, 360, &p.fov, None);
        assert!((none.0 - px).abs() < 1e-3);
        let ident = hud_origin(400, 360, &p.fov, Some(&crate::geometry::rot_y(0.0)));
        assert!((ident.0 - px).abs() < 1e-2 && (ident.1 - none.1).abs() < 1e-2);
        // eyes canted outward by 2 deg each: the copies move in opposite directions by tan(2 deg) in pixels
        let [tl, tr, _, _] = p.fov.tangents();
        let want = 2.0f32.to_radians().tan() / (tl + tr) * 400.0;
        let l = hud_origin(400, 360, &p.fov, Some(&crate::geometry::rot_y(2.0)));
        let r = hud_origin(400, 360, &p.fov, Some(&crate::geometry::rot_y(-2.0)));
        assert!(((l.0 - px).abs() - want).abs() < 0.05, "{} {want}", l.0 - px);
        assert!(((r.0 - px).abs() - want).abs() < 0.05);
        assert!((l.0 - px) * (r.0 - px) < 0.0, "opposite directions");
        // the drawn text follows: the left-most lit column moves by the same amount
        let lines = vec!["FUSE ME".to_string()];
        let first_col = |head: Option<&Mat3>| {
            let mut img = Image::new(400, 360, BLACK);
            draw_hud_at(&mut img, &p.fov, head, &lines);
            (0..400)
                .find(|&x| (0..360).any(|y| img.get(x, y) != [0, 0, 0]))
                .unwrap() as f32
        };
        let h = crate::geometry::rot_y(2.0);
        let d = first_col(Some(&h)) - first_col(None);
        assert!((d - (l.0 - px)).abs() <= 1.01, "{d} {}", l.0 - px);
        // the strip (swim) has the same shift
        let strip = |head: Option<&Mat3>| {
            let (_, im) = hud_strip_at(400, 360, &p.fov, head, &lines).unwrap();
            (0..400)
                .find(|&x| (0..im.h).any(|y| im.get(x, y) != [0, 0, 0]))
                .unwrap() as f32
        };
        assert_eq!(strip(Some(&h)) - strip(None), d);
    }

    #[test]
    fn hud_is_drawn_below_the_axis_and_inside_the_image() {
        let p = params(400, 360);
        let lines = vec!["LINE ONE".to_string(), "SECOND LINE IS LONGER".to_string()];
        let mut img = Image::new(400, 360, BLACK);
        draw_hud(&mut img, &p.fov, &lines);
        let lit: Vec<(u32, u32)> = (0..360)
            .flat_map(|y| (0..400).map(move |x| (x, y)))
            .filter(|&(x, y)| img.get(x, y) != [0, 0, 0])
            .collect();
        assert!(!lit.is_empty());
        // text colour is grey, not white
        assert!(
            img.count(|c| c == [184, 184, 184]) > 20,
            "{:?}",
            img.get(200, 300)
        );
        assert!(img.count(|c| c[0] > 190) == 0);
        // centred horizontally on the optical axis (x = 200) and below the middle of the image
        let minx = lit.iter().map(|p| p.0).min().unwrap();
        let maxx = lit.iter().map(|p| p.0).max().unwrap();
        assert!(((minx + maxx) as i32 / 2 - 200).abs() < 12, "{minx} {maxx}");
        assert!(lit.iter().all(|p| p.1 > 200));
        // a tiny image still does not panic
        let mut tiny = Image::new(20, 10, BLACK);
        draw_hud(&mut tiny, &p.fov, &lines);
        draw_hud(&mut tiny, &p.fov, &[]);
    }

    #[test]
    fn render_adds_labels_and_hud_and_blank_is_black() {
        let p = params(300, 270);
        let plain = render(Pattern::Rings, &p, None, 2);
        let with = render(Pattern::Rings, &p, Some(&["HELLO".to_string()]), 2);
        assert_ne!(plain, with);
        assert_eq!(plain, render(Pattern::Rings, &p, None, 1));
        let b = blank(10, 10);
        assert_eq!(b.count(|c| c == [0, 0, 0]), 100);
    }

    fn canted(eye: usize, deg: f32) -> Params {
        let h = deg.to_radians() * 0.5;
        let m = crate::geometry::eyes_to_head(
            [0.0, h.sin(), 0.0, h.cos()],
            [0.0, -h.sin(), 0.0, h.cos()],
        );
        let mut p = params(1200, 1100);
        p.eye = eye;
        p.head = Some(m[eye]);
        p
    }

    #[test]
    fn head_mode_puts_the_cross_on_head_forward_in_both_eyes() {
        for eye in 0..2 {
            let p = canted(eye, 2.0);
            let (hx, hy) = p.principal();
            let sign = if eye == 0 { 1.0 } else { -1.0 };
            let (ex, ey) = p
                .fov
                .tangent_to_pixel(p.w, p.h, sign * 2f32.to_radians().tan(), 0.0);
            assert!(
                (hx - ex).abs() < 0.5 && (hy - ey).abs() < 0.5,
                "{eye}: {hx} {ex}"
            );
            // the grid's red cross is there, and the optical axis only carries the faint marker
            let img = render(Pattern::Grid, &p, None, 2);
            let c = img.get(hx as u32, hy as u32 + 12);
            assert!(c[0] > 200 && c[1] < 60, "cross {c:?}");
            let (ax, ay) = p.fov.principal_point(p.w, p.h);
            let m = img.get(ax as u32, ay as u32 + 12);
            assert!(m[0] > m[2] + 15 && m[0] < 150, "marker {m:?}");
            // the eye-axis mode keeps the cross on the axis and has no marker
            let mut q = p.clone();
            q.head = None;
            let img = render(Pattern::Grid, &q, None, 2);
            assert!(img.get(ax as u32, ay as u32 + 12)[0] > 200);
        }
    }

    #[test]
    fn head_mode_labels_and_pixels_are_inverse_and_align_shift_converts_unchanged() {
        let p = canted(0, 3.0);
        for (x, y) in [(100, 200), (600, 550), (1100, 900)] {
            let (tx, ty) = p.tangent(x, y);
            let (px, py) = p.to_pixel(tx, ty);
            assert!((px - (x as f32 + 0.5)).abs() < 0.05 && (py - (y as f32 + 0.5)).abs() < 0.05);
        }
        // a vergence shift v of the head-centred target moves it on the eye image by tan(v) within 0.5 %,
        // so the lens.conf centre conversion (tan(v)/gain) of the align pattern is unchanged by head mode
        let plain = params(1200, 1100);
        for v in [0.05f32, 0.5, 2.0] {
            let t = v.to_radians().tan();
            let moved = p.to_pixel(t, 0.0).0 - p.principal().0;
            let want = plain.to_pixel(t, 0.0).0 - plain.principal().0;
            assert!((moved / want - 1.0).abs() < 0.005, "{v}: {moved} {want}");
        }
    }

    #[test]
    fn warp_moves_the_pattern_and_labels_follow() {
        let mut p = params(1200, 1100);
        p.warp = crate::tune::Tune {
            dk: 5,
            sc: 2,
            dk2: 3,
        }
        .warp(0.653);
        let (tx, ty) = p.tangent(1000, 300);
        let (px, py) = p.to_pixel(tx, ty);
        assert!((px - 1000.5).abs() < 0.1 && (py - 300.5).abs() < 0.1);
        let q = params(1200, 1100);
        assert_ne!(p.tangent(1000, 300), q.tangent(1000, 300));
        assert_eq!(
            p.tangent(600, 550).0.signum(),
            q.tangent(600, 550).0.signum()
        );
    }
}
