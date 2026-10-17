//! The two short animations of "Get the headset ready" when the Dream Air's HID chip is off: a finger presses the
//! power button, and the power plug comes out, waits 10 s and goes back in (the DisplayPort cable stays).
//! Simple line art drawn with egui shapes in the app palette (light and dark), looping on `t` seconds.
use crate::theme;
use eframe::egui::{self, epaint::PathStroke, Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Which animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anim {
    /// A finger presses the power button.
    Press,
    /// The power plug: out, 10 s, back in.
    Cable,
}

/// Length of one loop in seconds.
pub fn loop_secs(a: Anim) -> f64 {
    match a {
        Anim::Press => 3.0,
        Anim::Cable => 12.0,
    }
}

fn ease(x: f64) -> f32 {
    let x = x.clamp(0.0, 1.0);
    (x * x * (3.0 - 2.0 * x)) as f32
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

/// The drawing area.
pub const SIZE: Vec2 = Vec2::new(360.0, 170.0);

/// Draw the animation at time `t` (seconds) and keep the UI repainting.
pub fn show(ui: &mut Ui, anim: Anim, t: f64) {
    let (rect, _) = ui.allocate_exact_size(SIZE, Sense::hover());
    let p = theme::pal(ui);
    let line = Stroke::new(2.0, p.dim);
    let soft = Stroke::new(1.5, p.faint);
    let accent = Stroke::new(2.5, p.accent);
    let painter = ui.painter_at(rect);
    let o = rect.min.to_vec2();
    let pt = |x: f32, y: f32| Pos2::new(x, y) + o;
    let ph = (t % loop_secs(anim)) / loop_secs(anim);
    match anim {
        Anim::Press => {
            // the headset, seen from the front: the visor, two lenses, the strap
            let body = Rect::from_center_size(pt(150.0, 112.0), Vec2::new(210.0, 84.0));
            painter.rect_stroke(body, CornerRadius::same(34), line, StrokeKind::Middle);
            for x in [112.0, 188.0] {
                painter.circle_stroke(pt(x, 112.0), 21.0, line);
                painter.circle_stroke(pt(x, 112.0), 9.0, soft);
            }
            painter.line_segment([pt(150.0, 90.0), pt(150.0, 132.0)], soft);
            // the strap over the top
            let strap: Vec<Pos2> = (0..=16).map(|i| {
                let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
                pt(150.0 + 92.0 * a.cos(), 74.0 + 44.0 * a.sin())
            }).collect();
            painter.add(egui::Shape::line(strap, PathStroke::new(1.5, p.faint)));
            // the power button on the right of the top edge, pulsing
            let (bx, by) = (222.0, 66.0);
            let down = {
                let k = if ph < 0.35 { 0.0 } else if ph < 0.55 { 1.0 } else { 0.0 };
                k * 3.0
            };
            painter.rect_stroke(Rect::from_center_size(pt(bx, by + down), Vec2::new(22.0, 8.0)), CornerRadius::same(4), accent, StrokeKind::Middle);
            let pulse = (t % 1.0) as f32;
            painter.circle_stroke(pt(bx, by), 10.0 + 18.0 * pulse, Stroke::new(2.0, p.accent.gamma_multiply(1.0 - pulse)));
            // the finger: comes down, presses, goes up (the hand above it is clipped by the area)
            let tip = if ph < 0.4 {
                lerp(-6.0, 50.0, ease(ph / 0.4))
            } else if ph < 0.55 {
                50.0 + down
            } else if ph < 0.9 {
                lerp(50.0, -6.0, ease((ph - 0.55) / 0.35))
            } else {
                -6.0
            };
            let finger = Rect::from_min_max(pt(bx - 10.0, tip - 80.0), pt(bx + 10.0, tip));
            painter.rect_filled(finger, CornerRadius::same(10), p.card);
            painter.rect_stroke(finger, CornerRadius::same(10), line, StrokeKind::Middle);
            let hand = Rect::from_min_max(pt(bx - 30.0, tip - 150.0), pt(bx + 30.0, tip - 54.0));
            painter.rect_filled(hand, CornerRadius::same(16), p.card);
            painter.rect_stroke(hand, CornerRadius::same(16), line, StrokeKind::Middle);
            painter.text(pt(268.0, 120.0), Align2::LEFT_CENTER, "power\nbutton", FontId::proportional(12.0), p.faint);
        }
        Anim::Cable => {
            // the headset on the left, its power cable to the wall socket; the DisplayPort cable stays in
            let body = Rect::from_center_size(pt(62.0, 90.0), Vec2::new(86.0, 46.0));
            painter.rect_stroke(body, CornerRadius::same(18), line, StrokeKind::Middle);
            for x in [48.0, 76.0] {
                painter.circle_stroke(pt(x, 90.0), 9.0, soft);
            }
            let socket = Rect::from_min_max(pt(304.0, 66.0), pt(334.0, 114.0));
            painter.rect_stroke(socket, CornerRadius::same(6), line, StrokeKind::Middle);
            painter.circle_filled(pt(319.0, 80.0), 2.5, p.faint);
            painter.circle_filled(pt(319.0, 100.0), 2.5, p.faint);
            // 0..0.125 in, 0.125..0.21 out, ..0.875 out (the 10 s), ..0.96 back in, in
            let (inx, outx) = (284.0_f32, 214.0_f32);
            let (x, out) = if ph < 0.125 {
                (inx, false)
            } else if ph < 0.21 {
                (lerp(inx, outx, ease((ph - 0.125) / 0.085)), false)
            } else if ph < 0.875 {
                (outx, true)
            } else if ph < 0.96 {
                (lerp(outx, inx, ease((ph - 0.875) / 0.085)), false)
            } else {
                (inx, false)
            };
            // the cable sags more the further the plug is from the socket
            let slack = (inx - x) / (inx - outx);
            let a = pt(105.0, 90.0);
            let b = pt(x - 38.0, 90.0);
            let pts: Vec<Pos2> = (0..=24).map(|i| {
                let k = i as f32 / 24.0;
                Pos2::new(a.x + (b.x - a.x) * k, a.y + (b.y - a.y) * k + 18.0 * slack * (std::f32::consts::PI * k).sin())
            }).collect();
            painter.add(egui::Shape::line(pts, PathStroke::new(2.0, p.dim)));
            let plug = Rect::from_min_max(pt(x - 38.0, 78.0), pt(x, 102.0));
            painter.rect_filled(plug, CornerRadius::same(5), p.card);
            painter.rect_stroke(plug, CornerRadius::same(5), if out { accent } else { line }, StrokeKind::Middle);
            for dy in [-6.0_f32, 6.0] {
                painter.line_segment([pt(x, 90.0 + dy), pt(x + 14.0, 90.0 + dy)], if out { accent } else { line });
            }
            if out {
                // the 10 s: a ring that empties over the time the plug is out
                let k = ((ph - 0.21) / (0.875 - 0.21)) as f32;
                let c = pt(x - 19.0, 90.0 - 38.0);
                painter.circle_stroke(c, 13.0, Stroke::new(1.5, p.faint));
                let n = 32;
                let upto = ((1.0 - k) * n as f32) as usize;
                let arc: Vec<Pos2> = (0..=upto).map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / n as f32;
                    c + Vec2::new(a.cos(), a.sin()) * 13.0
                }).collect();
                if arc.len() > 1 {
                    painter.add(egui::Shape::line(arc, PathStroke::new(3.0, p.accent)));
                }
                painter.text(c, Align2::CENTER_CENTER, format!("{}", (10.0 * (1.0 - k)).ceil().max(0.0) as u32), FontId::proportional(12.0), p.text);
            }
            // the DisplayPort cable: stays in
            painter.line_segment([pt(62.0, 114.0), pt(62.0, 146.0)], Stroke::new(3.0, p.ok));
            painter.line_segment([pt(62.0, 146.0), pt(150.0, 146.0)], Stroke::new(3.0, p.ok));
            painter.text(pt(158.0, 146.0), Align2::LEFT_CENTER, "DisplayPort: leave it in", FontId::proportional(12.0), p.ok);
            painter.text(pt(319.0, 128.0), Align2::CENTER_TOP, "power", FontId::proportional(12.0), p.faint);
        }
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}
