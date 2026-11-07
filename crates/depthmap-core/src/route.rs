//! Routes: a curve between two nodes that passes through any number of
//! extra points. A route belongs to one depth; it is shown on that depth and
//! every deeper one, never on shallower ones.

use crate::node::NodeId;

pub const MAX_ROUTES: usize = 128;
pub const MAX_VIA: usize = 256;
pub const MAX_ROUTE_NAME: usize = 60;
pub const MIN_WIDTH: u8 = 1;
pub const MAX_WIDTH: u8 = 24;

pub const ROUTE_COLORS: [[u8; 3]; 6] = [
    [200, 60, 40],
    [40, 110, 200],
    [235, 175, 30],
    [150, 70, 170],
    [30, 150, 100],
    [60, 60, 60],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RouteStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl RouteStyle {
    pub const ALL: [RouteStyle; 3] = [RouteStyle::Solid, RouteStyle::Dashed, RouteStyle::Dotted];

    pub fn name(self) -> &'static str {
        match self {
            RouteStyle::Solid => "Solid",
            RouteStyle::Dashed => "Dashed",
            RouteStyle::Dotted => "Dotted",
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }

    pub fn from_byte(b: u8) -> Option<Self> {
        Self::ALL.get(b as usize).copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub name: String,
    pub color: [u8; 3],
    /// Line width in screen points.
    pub width: u8,
    pub style: RouteStyle,
    pub visible: bool,
    /// The depth the route was drawn on; `via` is in cells of this depth.
    pub depth: u8,
    pub from: NodeId,
    pub to: NodeId,
    /// Points the route must pass through, in order from `from` to `to`.
    pub via: Vec<(f64, f64)>,
}

impl Route {
    pub fn new(name: &str, color: [u8; 3], depth: u8, from: NodeId, to: NodeId) -> Self {
        Self {
            name: name.chars().take(MAX_ROUTE_NAME).collect(),
            color,
            width: 4,
            style: RouteStyle::Solid,
            visible: true,
            depth,
            from,
            to,
            via: Vec::new(),
        }
    }

    /// Whether the route is drawn when the camera is at `camera_depth`.
    pub fn shows_at(&self, camera_depth: u8) -> bool {
        self.visible && camera_depth >= self.depth
    }

    /// Inserts a point where it bends the route least: between the control
    /// points whose curve is nearest to `p`. `ends` are the node positions.
    /// Returns the index it was inserted at.
    pub fn insert_via(&mut self, ends: ((f64, f64), (f64, f64)), p: (f64, f64)) -> Option<usize> {
        if self.via.len() >= MAX_VIA {
            return None;
        }
        let ctrl = self.control_points(ends);
        let curve = spline(&ctrl, 16);
        let i = curve.nearest_segment(p);
        self.via.insert(i, p);
        Some(i)
    }

    /// `from`, the via points, `to`.
    pub fn control_points(&self, ends: ((f64, f64), (f64, f64))) -> Vec<(f64, f64)> {
        let mut v = Vec::with_capacity(self.via.len() + 2);
        v.push(ends.0);
        v.extend(&self.via);
        v.push(ends.1);
        v
    }
}

/// A sampled curve and, for every sample, the control segment it lies on.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub points: Vec<(f64, f64)>,
    pub segment: Vec<usize>,
}

impl Curve {
    /// The control segment (0-based, from control point `i` to `i + 1`) of
    /// the sample nearest to `p`.
    pub fn nearest_segment(&self, p: (f64, f64)) -> usize {
        let d = |q: &(f64, f64)| (q.0 - p.0).powi(2) + (q.1 - p.1).powi(2);
        self.points
            .iter()
            .enumerate()
            .min_by(|a, b| d(a.1).partial_cmp(&d(b.1)).unwrap())
            .map_or(0, |(i, _)| self.segment[i])
    }

    /// Distance from `p` to the curve (to the polyline of its samples).
    pub fn distance_to(&self, p: (f64, f64)) -> f64 {
        self.points
            .windows(2)
            .map(|w| dist_to_segment(p, w[0], w[1]))
            .fold(f64::MAX, f64::min)
    }
}

fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    ((p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)).abs()
}

/// A smooth curve through every control point (centripetal Catmull-Rom, so
/// it neither loops nor overshoots on unevenly spaced points).
/// `per_segment` samples are taken between each pair of control points.
pub fn spline(ctrl: &[(f64, f64)], per_segment: usize) -> Curve {
    let n = per_segment.max(1);
    let mut curve = Curve { points: Vec::new(), segment: Vec::new() };
    match ctrl.len() {
        0 => return curve,
        1 => {
            curve.points.push(ctrl[0]);
            curve.segment.push(0);
            return curve;
        }
        _ => {}
    }
    let reflect = |a: (f64, f64), b: (f64, f64)| (2.0 * a.0 - b.0, 2.0 * a.1 - b.1);
    for i in 0..ctrl.len() - 1 {
        let p1 = ctrl[i];
        let p2 = ctrl[i + 1];
        let p0 = if i == 0 { reflect(p1, p2) } else { ctrl[i - 1] };
        let p3 = if i + 2 < ctrl.len() { ctrl[i + 2] } else { reflect(p2, p1) };
        let knot = |a: (f64, f64), b: (f64, f64)| ((b.0 - a.0).hypot(b.1 - a.1)).sqrt().max(1e-6);
        let t0 = 0.0;
        let t1 = t0 + knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        let t3 = t2 + knot(p2, p3);
        let lerp = |a: (f64, f64), b: (f64, f64), ta: f64, tb: f64, t: f64| {
            let w = (t - ta) / (tb - ta);
            (a.0 + (b.0 - a.0) * w, a.1 + (b.1 - a.1) * w)
        };
        for s in 0..n {
            let t = t1 + (t2 - t1) * s as f64 / n as f64;
            let a1 = lerp(p0, p1, t0, t1, t);
            let a2 = lerp(p1, p2, t1, t2, t);
            let a3 = lerp(p2, p3, t2, t3, t);
            let b1 = lerp(a1, a2, t0, t2, t);
            let b2 = lerp(a2, a3, t1, t3, t);
            curve.points.push(lerp(b1, b2, t1, t2, t));
            curve.segment.push(i);
        }
    }
    curve.points.push(*ctrl.last().unwrap());
    curve.segment.push(ctrl.len() - 2);
    curve
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn the_curve_passes_through_every_control_point() {
        let ctrl = [(0.0, 0.0), (10.0, 5.0), (12.0, -4.0), (30.0, 8.0), (31.0, 9.0)];
        let c = spline(&ctrl, 12);
        for p in ctrl {
            assert!(c.points.iter().any(|&q| close(p, q)), "missing {p:?}");
        }
        assert!(close(c.points[0], ctrl[0]) && close(*c.points.last().unwrap(), ctrl[4]));
        assert_eq!(c.points.len(), 12 * 4 + 1);
    }

    #[test]
    fn two_points_give_a_straight_line_and_collinear_points_stay_straight() {
        let c = spline(&[(0.0, 0.0), (10.0, 0.0)], 5);
        assert!(c.points.iter().all(|p| p.1.abs() < 1e-9));
        let c = spline(&[(0.0, 0.0), (4.0, 4.0), (9.0, 9.0)], 8);
        assert!(c.points.iter().all(|p| (p.0 - p.1).abs() < 1e-9));
    }

    #[test]
    fn repeated_points_do_not_break_the_curve() {
        let c = spline(&[(1.0, 1.0), (1.0, 1.0), (5.0, 5.0)], 6);
        assert!(c.points.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
    }

    #[test]
    fn inserting_a_point_picks_the_nearest_stretch() {
        let mut r = Route::new("Road", ROUTE_COLORS[0], 0, 1, 2);
        let ends = ((0.0, 0.0), (30.0, 0.0));
        assert_eq!(r.insert_via(ends, (10.0, 3.0)), Some(0));
        assert_eq!(r.insert_via(ends, (25.0, 3.0)), Some(1), "after the first via point");
        assert_eq!(r.insert_via(ends, (18.0, -3.0)), Some(1), "between the two");
        assert_eq!(r.via, vec![(10.0, 3.0), (18.0, -3.0), (25.0, 3.0)]);
    }

    #[test]
    fn distance_to_the_curve_and_visibility_by_depth() {
        let c = spline(&[(0.0, 0.0), (10.0, 0.0)], 10);
        assert!((c.distance_to((5.0, 3.0)) - 3.0).abs() < 1e-9);
        let mut r = Route::new("R", [0; 3], 2, 1, 2);
        assert!(!r.shows_at(1) && r.shows_at(2) && r.shows_at(5), "bleeds down, never up");
        r.visible = false;
        assert!(!r.shows_at(3));
    }
}
