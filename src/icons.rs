//! Remoport's own line icons, drawn as vectors on a 24×24 grid so they stay
//! sharp at any size and do not depend on which symbols a font has.

use egui::epaint::{PathShape, Shape};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Rounding, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Star,
    StarFilled,
    Monitor,
    Window,
    Plus,
    Gear,
    Feedback,
    Help,
    Search,
    More,
    ChevronDown,
    ChevronRight,
    Person,
}

/// Paints `icon` to fill `rect` (best square) in `color`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let unit = side / 24.0;
    let origin = rect.center() - egui::vec2(side / 2.0, side / 2.0);
    let p = |x: f32, y: f32| origin + egui::vec2(x * unit, y * unit);
    let stroke = Stroke::new((1.6 * unit).max(1.0), color);
    let line = |points: Vec<Pos2>| Shape::Path(PathShape::line(points, stroke));
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));

    match icon {
        Icon::Star | Icon::StarFilled => {
            let center = p(12.0, 12.6);
            let point = |i: usize, radius: f32| {
                let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                center + egui::vec2(angle.cos(), angle.sin()) * radius * unit
            };
            let outline: Vec<Pos2> = (0..10)
                .map(|i| point(i, if i % 2 == 0 { 10.0 } else { 4.3 }))
                .collect();
            if icon == Icon::StarFilled {
                // A star is not convex: fill the inner pentagon and the five tips.
                let inner: Vec<Pos2> = (0..5).map(|i| outline[2 * i + 1]).collect();
                painter.add(Shape::convex_polygon(inner, color, Stroke::NONE));
                for i in 0..5 {
                    let tip = vec![
                        outline[(2 * i + 9) % 10],
                        outline[2 * i],
                        outline[2 * i + 1],
                    ];
                    painter.add(Shape::convex_polygon(tip, color, Stroke::NONE));
                }
            } else {
                painter.add(Shape::Path(PathShape::closed_line(outline, stroke)));
            }
        }
        Icon::Monitor => {
            painter.rect_stroke(r(3.0, 4.0, 21.0, 16.0), Rounding::same(2.0 * unit), stroke);
            painter.add(line(vec![p(12.0, 16.0), p(12.0, 19.5)]));
            painter.add(line(vec![p(8.0, 20.0), p(16.0, 20.0)]));
        }
        Icon::Window => {
            painter.rect_stroke(r(3.0, 5.0, 21.0, 19.0), Rounding::same(2.0 * unit), stroke);
            painter.add(line(vec![p(3.0, 9.0), p(21.0, 9.0)]));
            painter.circle_filled(p(6.0, 7.0), 0.9 * unit, color);
            painter.circle_filled(p(8.8, 7.0), 0.9 * unit, color);
        }
        Icon::Plus => {
            painter.add(line(vec![p(12.0, 5.0), p(12.0, 19.0)]));
            painter.add(line(vec![p(5.0, 12.0), p(19.0, 12.0)]));
        }
        Icon::Gear => {
            let center = p(12.0, 12.0);
            painter.circle_stroke(center, 3.0 * unit, stroke);
            painter.circle_stroke(center, 7.0 * unit, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                let dir = egui::vec2(angle.cos(), angle.sin());
                painter.add(line(vec![
                    center + dir * 7.0 * unit,
                    center + dir * 9.5 * unit,
                ]));
            }
        }
        Icon::Feedback => {
            painter.rect_stroke(r(3.0, 4.0, 21.0, 16.0), Rounding::same(3.0 * unit), stroke);
            painter.add(line(vec![p(8.0, 16.0), p(7.0, 20.5), p(12.5, 16.0)]));
            painter.add(line(vec![p(7.5, 8.5), p(16.5, 8.5)]));
            painter.add(line(vec![p(7.5, 11.5), p(13.5, 11.5)]));
        }
        Icon::Help => {
            painter.circle_stroke(p(12.0, 12.0), 9.0 * unit, stroke);
            painter.text(
                p(12.0, 12.3),
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(13.0 * unit),
                color,
            );
        }
        Icon::Search => {
            painter.circle_stroke(p(10.5, 10.5), 6.0 * unit, stroke);
            painter.add(line(vec![p(15.0, 15.0), p(20.0, 20.0)]));
        }
        Icon::More => {
            for x in [6.0, 12.0, 18.0] {
                painter.circle_filled(p(x, 12.0), 1.6 * unit, color);
            }
        }
        Icon::ChevronDown => {
            painter.add(line(vec![p(7.0, 9.5), p(12.0, 14.5), p(17.0, 9.5)]));
        }
        Icon::ChevronRight => {
            painter.add(line(vec![p(9.5, 7.0), p(14.5, 12.0), p(9.5, 17.0)]));
        }
        Icon::Person => {
            painter.circle_stroke(p(12.0, 8.5), 3.8 * unit, stroke);
            let shoulders: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let angle = std::f32::consts::PI + i as f32 * std::f32::consts::PI / 16.0;
                    p(12.0 + 7.0 * angle.cos(), 20.5 + 6.5 * angle.sin())
                })
                .collect();
            painter.add(line(shoulders));
        }
    }
}

/// Remoport's logo mark: a teal tile with a window pane and a way in.
/// Mirrors `data/icons/hicolor/scalable/apps/app.remoport.Remoport.svg`.
pub fn paint_logo(painter: &Painter, rect: Rect) {
    let unit = rect.width() / 128.0;
    let p = |x: f32, y: f32| rect.min + egui::vec2(x * unit, y * unit);
    let tile = Rect::from_min_max(p(8.0, 8.0), p(120.0, 120.0));
    painter.rect_filled(
        tile,
        Rounding::same(26.0 * unit),
        Color32::from_rgb(0x1f, 0xa8, 0x9b),
    );
    let white = Stroke::new(7.0 * unit, Color32::WHITE);
    painter.rect_stroke(
        Rect::from_min_max(p(28.0, 32.0), p(100.0, 84.0)),
        Rounding::same(8.0 * unit),
        white,
    );
    painter.add(Shape::Path(PathShape::line(
        vec![p(52.0, 100.0), p(76.0, 100.0)],
        white,
    )));
    painter.add(Shape::Path(PathShape::line(
        vec![p(50.0, 58.0), p(72.0, 58.0)],
        white,
    )));
    painter.add(Shape::Path(PathShape::line(
        vec![p(63.0, 48.0), p(73.0, 58.0), p(63.0, 68.0)],
        white,
    )));
}

/// A rectangle shaded diagonally from `from` (top left) to `to` (bottom right).
pub fn gradient_rect(rect: Rect, from: Color32, to: Color32) -> Shape {
    let mid = lerp(from, to, 0.5);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), from);
    mesh.colored_vertex(rect.right_top(), mid);
    mesh.colored_vertex(rect.right_bottom(), to);
    mesh.colored_vertex(rect.left_bottom(), mid);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
