//! Classic gate symbols as polygons in a 66×50 box (the same shapes the SVG
//! renderer draws), plus a small triangulator so egui can fill them even
//! though Or and Xor bodies aren't convex.

use egui::{Pos2, pos2};
use wafer_core::GateKind;

pub struct GateShape {
    /// The closed outline of the gate body.
    pub body: Vec<Pos2>,
    /// Xor's extra curve behind the body.
    pub extra: Option<Vec<Pos2>>,
    /// Where input stubs meet the body.
    pub back_x: f32,
    /// The rightmost point of the body, where the output (or bubble) starts.
    pub tip: f32,
    pub bubble: bool,
}

pub fn gate_shape(kind: GateKind) -> GateShape {
    let bubble = matches!(kind, GateKind::Nand | GateKind::Nor | GateKind::Xnor | GateKind::Not);
    match kind {
        GateKind::And | GateKind::Nand => {
            let mut body = vec![pos2(0.0, 0.0), pos2(30.0, 0.0)];
            for i in 1..24 {
                let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 24.0;
                body.push(pos2(30.0 + 25.0 * angle.cos(), 25.0 + 25.0 * angle.sin()));
            }
            body.extend([pos2(30.0, 50.0), pos2(0.0, 50.0)]);
            GateShape { body, extra: None, back_x: 0.0, tip: 55.0, bubble }
        }
        GateKind::Or | GateKind::Nor => {
            GateShape { body: or_body(0.0, 14.0, 36.0, 55.0), extra: None, back_x: 6.0, tip: 55.0, bubble }
        }
        GateKind::Xor | GateKind::Xnor => GateShape {
            body: or_body(6.0, 19.0, 38.0, 56.0),
            extra: Some(quad(pos2(0.0, 0.0), pos2(13.0, 25.0), pos2(0.0, 50.0))),
            back_x: 11.5,
            tip: 56.0,
            bubble,
        },
        GateKind::Not => GateShape {
            body: vec![pos2(4.0, 7.0), pos2(50.0, 25.0), pos2(4.0, 43.0)],
            extra: None,
            back_x: 4.0,
            tip: 50.0,
            bubble,
        },
    }
}

/// An Or-shaped body: a curved back at `back`, bulging to `bulge`, with the
/// front curves meeting at `tip`.
fn or_body(back: f32, bulge: f32, shoulder: f32, tip: f32) -> Vec<Pos2> {
    let mut points = quad(pos2(back, 0.0), pos2(bulge, 25.0), pos2(back, 50.0));
    points.extend(quad(pos2(back, 50.0), pos2(shoulder, 50.0), pos2(tip, 25.0)).into_iter().skip(1));
    points.extend(quad(pos2(tip, 25.0), pos2(shoulder, 0.0), pos2(back, 0.0)).into_iter().skip(1));
    points.pop(); // The last point repeats the first.
    points
}

/// Samples a quadratic Bézier curve.
fn quad(p0: Pos2, p1: Pos2, p2: Pos2) -> Vec<Pos2> {
    (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            let u = 1.0 - t;
            pos2(u * u * p0.x + 2.0 * u * t * p1.x + t * t * p2.x, u * u * p0.y + 2.0 * u * t * p1.y + t * t * p2.y)
        })
        .collect()
}

/// Splits a simple polygon into triangles by ear clipping. Returns indices
/// into `points`, three per triangle.
pub fn triangulate(points: &[Pos2]) -> Vec<u32> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let orientation = area.signum();
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
    let inside = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        (d1 * orientation > 0.0) && (d2 * orientation > 0.0) && (d3 * orientation > 0.0)
    };

    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity((n - 2) * 3);
    while remaining.len() > 3 {
        let m = remaining.len();
        let ear = (0..m).find(|&i| {
            let (a, b, c) = (remaining[(i + m - 1) % m], remaining[i], remaining[(i + 1) % m]);
            let (pa, pb, pc) = (points[a], points[b], points[c]);
            cross(pa, pb, pc) * orientation > 0.0
                && !remaining.iter().any(|&o| o != a && o != b && o != c && inside(points[o], pa, pb, pc))
        });
        let Some(i) = ear else { break };
        let m = remaining.len();
        triangles.extend([remaining[(i + m - 1) % m], remaining[i], remaining[(i + 1) % m]].map(|v| v as u32));
        remaining.remove(i);
    }
    if remaining.len() == 3 {
        triangles.extend(remaining.iter().map(|&v| v as u32));
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[Pos2], tris: &[u32]) -> f32 {
        tris.chunks(3)
            .map(|t| {
                let (a, b, c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
                ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs() / 2.0
            })
            .sum()
    }

    #[test]
    fn every_gate_triangulates_fully() {
        for kind in GateKind::ALL {
            let shape = gate_shape(kind);
            let tris = triangulate(&shape.body);
            assert_eq!(tris.len(), (shape.body.len() - 2) * 3, "{kind:?} left gaps");
            assert!(area(&shape.body, &tris) > 500.0, "{kind:?}");
        }
    }
}
