//! Vector icons, drawn with the painter so they are sharp at any size and need no font.

use eframe::egui::{Color32, Painter, Pos2, Shape, Stroke, Vec2, epaint::CubicBezierShape, pos2, vec2};
use std::f32::consts::PI;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Phone,
    HangUp,
    Mic,
    MicOff,
    Pause,
    Keypad,
    Backspace,
    Gear,
    Search,
    Eye,
}

/// Draws `icon` centered on `c`, about `s` points across.
pub fn draw(p: &Painter, icon: Icon, c: Pos2, s: f32, col: Color32) {
    let w = (s * 0.11).max(1.4);
    let line = Stroke::new(w, col);
    match icon {
        Icon::Phone => handset(p, c, s, col, 0.0),
        Icon::HangUp => handset(p, c, s, col, 3.0 * PI / 4.0),
        Icon::Mic | Icon::MicOff => {
            let body = eframe::egui::Rect::from_center_size(c - vec2(0.0, s * 0.12), vec2(s * 0.34, s * 0.52));
            p.rect_stroke(body, (s * 0.17) as u8, line, eframe::egui::StrokeKind::Middle);
            // The cradle under it, and the stand.
            let pts = arc(c - vec2(0.0, s * 0.10), s * 0.32, 0.0, PI, 14);
            p.add(Shape::line(pts, line));
            p.line_segment([c + vec2(0.0, s * 0.22), c + vec2(0.0, s * 0.38)], line);
            p.line_segment([c + vec2(-s * 0.14, s * 0.38), c + vec2(s * 0.14, s * 0.38)], line);
            if icon == Icon::MicOff {
                p.line_segment([c + vec2(-s * 0.36, -s * 0.38), c + vec2(s * 0.36, s * 0.36)], Stroke::new(w * 1.1, col));
            }
        }
        Icon::Pause => {
            for dx in [-0.13, 0.13] {
                let r = eframe::egui::Rect::from_center_size(c + vec2(s * dx, 0.0), vec2(s * 0.13, s * 0.5));
                p.rect_filled(r, (s * 0.05) as u8, col);
            }
        }
        Icon::Keypad => {
            for row in 0..3 {
                for colm in 0..3 {
                    p.circle_filled(c + vec2((colm as f32 - 1.0) * s * 0.26, (row as f32 - 1.0) * s * 0.26), s * 0.07, col);
                }
            }
        }
        Icon::Backspace => {
            let (l, r, t, b) = (c.x - s * 0.42, c.x + s * 0.38, c.y - s * 0.26, c.y + s * 0.26);
            let pts = vec![pos2(l, c.y), pos2(l + s * 0.22, t), pos2(r, t), pos2(r, b), pos2(l + s * 0.22, b)];
            p.add(Shape::closed_line(pts, line));
            let x = c + vec2(s * 0.07, 0.0);
            let d = s * 0.11;
            p.line_segment([x + vec2(-d, -d), x + vec2(d, d)], line);
            p.line_segment([x + vec2(-d, d), x + vec2(d, -d)], line);
        }
        Icon::Gear => {
            for k in 0..8 {
                let a = k as f32 * PI / 4.0;
                let d = Vec2::angled(a);
                p.line_segment([c + d * s * 0.26, c + d * s * 0.42], Stroke::new(w * 1.6, col));
            }
            p.circle_stroke(c, s * 0.27, line);
            p.circle_stroke(c, s * 0.10, line);
        }
        Icon::Search => {
            let o = c - vec2(s * 0.07, s * 0.07);
            p.circle_stroke(o, s * 0.24, line);
            p.line_segment([o + vec2(s * 0.18, s * 0.18), c + vec2(s * 0.36, s * 0.36)], Stroke::new(w * 1.3, col));
        }
        Icon::Eye => {
            let top = arc(c + vec2(0.0, s * 0.42), s * 0.62, -PI * 0.78, -PI * 0.22, 16);
            let bottom = arc(c - vec2(0.0, s * 0.42), s * 0.62, PI * 0.22, PI * 0.78, 16);
            p.add(Shape::line(top, line));
            p.add(Shape::line(bottom, line));
            p.circle_filled(c, s * 0.12, col);
        }
    }
}

/// A handset: a curved grip with the ear and mouth pieces at its ends, turned by `rot`.
fn handset(p: &Painter, c: Pos2, s: f32, col: Color32, rot: f32) {
    let r = |v: Vec2| c + rotate(v, rot);
    let ear = r(vec2(-s * 0.26, -s * 0.30));
    let mouth = r(vec2(s * 0.30, s * 0.26));
    let curve = CubicBezierShape::from_points_stroke(
        [ear, r(vec2(-s * 0.36, s * 0.05)), r(vec2(-s * 0.05, s * 0.36)), mouth],
        false,
        Color32::TRANSPARENT,
        Stroke::new(s * 0.16, col),
    );
    p.add(curve);
    // The pieces: short thick strokes across the ends.
    for (end, dir) in [(ear, rotate(vec2(1.0, -0.55), rot)), (mouth, rotate(vec2(0.55, -1.0), rot))] {
        let d = dir.normalized() * s * 0.11;
        p.line_segment([end - d, end + d], Stroke::new(s * 0.20, col));
        p.circle_filled(end - d, s * 0.10, col);
        p.circle_filled(end + d, s * 0.10, col);
    }
}

fn rotate(v: Vec2, a: f32) -> Vec2 {
    let (sn, cs) = a.sin_cos();
    vec2(v.x * cs - v.y * sn, v.x * sn + v.y * cs)
}

fn arc(c: Pos2, r: f32, from: f32, to: f32, n: usize) -> Vec<Pos2> {
    (0..=n).map(|i| c + Vec2::angled(from + (to - from) * i as f32 / n as f32) * r).collect()
}
