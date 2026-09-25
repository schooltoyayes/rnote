//! Recognizes shapes in freehand strokes, so a stroke can be replaced by a clean shape: lines,
//! polylines, circles, ellipses, triangles, rectangles and other polygons.

// Imports
use p2d::glamx::DAffine2;
use p2d::math::Vector2;
use p2d::shape::Cuboid;
use rnote_compose::shapes::{Ellipse, Line, Polygon, Polyline, Rectangle, Shape};

/// The maximum distance of the drawn points from a recognized straight shape, relative to the
/// size of the stroke.
const MAX_DEVIATION: f64 = 0.08;
/// The ends of a stroke closer than this, relative to its length, make it closed.
const CLOSED_GAP: f64 = 0.15;
/// The maximum mean deviation of the drawn points from a recognized ellipse, relative to its
/// radii.
const MAX_ELLIPSE_ERROR: f64 = 0.07;
/// Ellipses with an axis ratio above this become circles.
const CIRCLE_RATIO: f64 = 0.85;
/// Lines and axes within this angle of horizontal or vertical are made horizontal or vertical.
const AXIS_SNAP_DEG: f64 = 5.0;
/// Quadrilaterals with all corners within this angle of a right angle become rectangles.
const RIGHT_ANGLE_TOLERANCE_DEG: f64 = 12.0;
/// Corners that turn less than this are left out.
const MIN_CORNER_TURN_DEG: f64 = 25.0;
const MAX_POLYGON_CORNERS: usize = 8;
const MAX_POLYLINE_CORNERS: usize = 5;
/// The number of points closed strokes are resampled to, for fitting ellipses.
const RESAMPLE_COUNT: usize = 72;

/// Recognize a shape in the points of a freehand stroke.
///
/// Returns `None` when the stroke does not look like one of the shapes.
pub fn recognize(points: &[Vector2]) -> Option<Shape> {
    let mut points = points.to_vec();
    points.dedup_by(|a, b| (*a - *b).length() < 1e-6);
    if points.len() < 2 {
        return None;
    }
    let size = bounds_diagonal(&points);
    let length = path_length(&points);
    if size < 1e-3 {
        return None;
    }

    let gap = (points[0] - points[points.len() - 1]).length();
    if gap > CLOSED_GAP * length {
        recognize_open(&points, size)
    } else {
        recognize_closed(&points, size)
    }
}

fn recognize_open(points: &[Vector2], size: f64) -> Option<Shape> {
    let (start, end) = (points[0], points[points.len() - 1]);
    let max_deviation = MAX_DEVIATION * size;

    if points
        .iter()
        .all(|p| distance_to_segment(*p, start, end) <= max_deviation)
    {
        return Some(Shape::Line(Line::new(start, snap_line_end(start, end))));
    }

    let corners = remove_flat_corners(rdp(points, max_deviation), false);
    if corners.len() >= 3 && corners.len() <= MAX_POLYLINE_CORNERS + 2 {
        return Some(Shape::Polyline(Polyline {
            start: corners[0],
            path: corners[1..].to_vec(),
        }));
    }
    None
}

fn recognize_closed(points: &[Vector2], size: f64) -> Option<Shape> {
    let resampled = resample_closed(points, RESAMPLE_COUNT);
    if let Some(ellipse) = fit_ellipse(&resampled) {
        return Some(Shape::Ellipse(ellipse));
    }

    let corners = remove_flat_corners(simplify_closed(&resampled, MAX_DEVIATION * size), true);
    match corners.len() {
        4 => Some(
            rectangle_from_corners(&corners)
                .map(Shape::Rectangle)
                .unwrap_or_else(|| polygon(&corners)),
        ),
        3..=MAX_POLYGON_CORNERS => Some(polygon(&corners)),
        _ => None,
    }
}

fn polygon(corners: &[Vector2]) -> Shape {
    Shape::Polygon(Polygon {
        start: corners[0],
        path: corners[1..].to_vec(),
    })
}

/// Fit an ellipse along the principal axes of the points, which must be spread evenly along
/// the stroke. `None` if the points don't lie on it closely enough.
fn fit_ellipse(points: &[Vector2]) -> Option<Ellipse> {
    let n = points.len() as f64;
    let centroid = points.iter().sum::<Vector2>() / n;
    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for p in points {
        let d = *p - centroid;
        cxx += d.x * d.x;
        cyy += d.y * d.y;
        cxy += d.x * d.y;
    }
    let angle = snap_axis_angle(0.5 * (2.0 * cxy).atan2(cxx - cyy));
    let (axis_u, axis_v) = (
        Vector2::new(angle.cos(), angle.sin()),
        Vector2::new(-angle.sin(), angle.cos()),
    );

    // The extents along the axes, which hand drawn strokes follow more closely than their
    // variance.
    let (mut min, mut max) = (
        Vector2::splat(f64::INFINITY),
        Vector2::splat(f64::NEG_INFINITY),
    );
    for p in points {
        let d = *p - centroid;
        let local = Vector2::new(d.dot(axis_u), d.dot(axis_v));
        min = min.min(local);
        max = max.max(local);
    }
    let mut radii = (max - min) * 0.5;
    if radii.min_element() < 1e-3 {
        return None;
    }
    let local_center = (max + min) * 0.5;
    let center = centroid + local_center.x * axis_u + local_center.y * axis_v;

    let error = points
        .iter()
        .map(|p| {
            let d = *p - center;
            let local = Vector2::new(d.dot(axis_u), d.dot(axis_v));
            ((local / radii).length() - 1.0).abs()
        })
        .sum::<f64>()
        / n;
    if error > MAX_ELLIPSE_ERROR {
        return None;
    }

    let angle = if radii.min_element() / radii.max_element() >= CIRCLE_RATIO {
        radii = Vector2::splat((radii.x + radii.y) * 0.5);
        0.0
    } else {
        angle
    };
    Some(Ellipse {
        radii,
        affine: DAffine2::from_angle_translation(angle, center),
    })
}

/// A rectangle, if all corners are close to right angles. It is aligned to the average
/// direction of the edges.
fn rectangle_from_corners(corners: &[Vector2]) -> Option<Rectangle> {
    let n = corners.len();
    for i in 0..n {
        let (prev, corner, next) = (corners[(i + n - 1) % n], corners[i], corners[(i + 1) % n]);
        let angle = (prev - corner).angle_to(next - corner).abs().to_degrees();
        if (angle - 90.0).abs() > RIGHT_ANGLE_TOLERANCE_DEG {
            return None;
        }
    }

    // Average the edge directions, turned into the same quarter by quadrupling the angles.
    let sum = (0..n)
        .map(|i| {
            let edge = corners[(i + 1) % n] - corners[i];
            let angle = 4.0 * edge.y.atan2(edge.x);
            Vector2::new(angle.cos(), angle.sin()) * edge.length()
        })
        .sum::<Vector2>();
    let angle = snap_axis_angle(sum.y.atan2(sum.x) / 4.0);
    let (axis_u, axis_v) = (
        Vector2::new(angle.cos(), angle.sin()),
        Vector2::new(-angle.sin(), angle.cos()),
    );

    let (mut min, mut max) = (
        Vector2::splat(f64::INFINITY),
        Vector2::splat(f64::NEG_INFINITY),
    );
    for corner in corners {
        let local = Vector2::new(corner.dot(axis_u), corner.dot(axis_v));
        min = min.min(local);
        max = max.max(local);
    }
    let local_center = (max + min) * 0.5;
    Some(Rectangle {
        cuboid: Cuboid::new((max - min) * 0.5),
        affine: DAffine2::from_angle_translation(
            angle,
            local_center.x * axis_u + local_center.y * axis_v,
        ),
    })
}

/// Snap an angle to horizontal or vertical when it is close to them.
fn snap_axis_angle(angle: f64) -> f64 {
    let quarter = std::f64::consts::FRAC_PI_2;
    let snapped = (angle / quarter).round() * quarter;
    if (angle - snapped).abs() <= AXIS_SNAP_DEG.to_radians() {
        snapped
    } else {
        angle
    }
}

/// The end of a line from `start` to `end`, made horizontal or vertical when it is close to.
fn snap_line_end(start: Vector2, end: Vector2) -> Vector2 {
    let d = end - start;
    let angle = snap_axis_angle(d.y.atan2(d.x));
    start + Vector2::new(angle.cos(), angle.sin()) * d.length()
}

/// Leave out corners which barely turn, and with `closed` also look at the first and the last
/// point as corners.
fn remove_flat_corners(mut corners: Vec<Vector2>, closed: bool) -> Vec<Vector2> {
    loop {
        let n = corners.len();
        if n < 3 {
            return corners;
        }
        let range = if closed { 0..n } else { 1..n - 1 };
        let flat = range.into_iter().find(|&i| {
            let (prev, corner, next) = (corners[(i + n - 1) % n], corners[i], corners[(i + 1) % n]);
            (corner - prev).angle_to(next - corner).abs().to_degrees() < MIN_CORNER_TURN_DEG
        });
        match flat {
            Some(i) => {
                corners.remove(i);
            }
            None => return corners,
        }
    }
}

/// Simplify a closed stroke to its corners. Starts at the point farthest from the centroid,
/// which is likely a corner.
fn simplify_closed(points: &[Vector2], epsilon: f64) -> Vec<Vector2> {
    let centroid = points.iter().sum::<Vector2>() / points.len() as f64;
    let first = (0..points.len())
        .max_by(|&a, &b| {
            (points[a] - centroid)
                .length()
                .total_cmp(&(points[b] - centroid).length())
        })
        .unwrap_or(0);
    let mut loop_points = points[first..].to_vec();
    loop_points.extend_from_slice(&points[..=first]);

    let mut simplified = rdp(&loop_points, epsilon);
    // The last point is the first one again.
    simplified.pop();
    simplified
}

/// Simplify a polyline with the Ramer-Douglas-Peucker algorithm.
fn rdp(points: &[Vector2], epsilon: f64) -> Vec<Vector2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (start, end) = (points[0], points[points.len() - 1]);
    let (index, dist) = points[1..points.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, distance_to_segment(*p, start, end)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.0));

    if dist <= epsilon {
        return vec![start, end];
    }
    let mut left = rdp(&points[..=index], epsilon);
    let right = rdp(&points[index..], epsilon);
    left.pop();
    left.extend(right);
    left
}

/// Resample a closed stroke to `count` points evenly spread along it.
fn resample_closed(points: &[Vector2], count: usize) -> Vec<Vector2> {
    let mut closed = points.to_vec();
    closed.push(points[0]);
    let length = path_length(&closed);
    let step = length / count as f64;

    let mut resampled = Vec::with_capacity(count);
    let mut segment = 0;
    let mut segment_start = 0.0;
    for i in 0..count {
        let target = i as f64 * step;
        while segment < closed.len() - 2
            && segment_start + (closed[segment + 1] - closed[segment]).length() < target
        {
            segment_start += (closed[segment + 1] - closed[segment]).length();
            segment += 1;
        }
        let (a, b) = (closed[segment], closed[segment + 1]);
        let segment_length = (b - a).length();
        let t = if segment_length > 0.0 {
            ((target - segment_start) / segment_length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        resampled.push(a + (b - a) * t);
    }
    resampled
}

fn distance_to_segment(p: Vector2, a: Vector2, b: Vector2) -> f64 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-12 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn path_length(points: &[Vector2]) -> f64 {
    points.windows(2).map(|w| (w[1] - w[0]).length()).sum()
}

fn bounds_diagonal(points: &[Vector2]) -> f64 {
    let (min, max) = points.iter().fold(
        (
            Vector2::splat(f64::INFINITY),
            Vector2::splat(f64::NEG_INFINITY),
        ),
        |(min, max), p| (min.min(*p), max.max(*p)),
    );
    (max - min).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A little wobble, like a hand drawn stroke.
    fn wobble(i: usize, amount: f64) -> Vector2 {
        let i = i as f64;
        Vector2::new((i * 1.7).sin(), (i * 2.3).cos()) * amount
    }

    /// Points along the corners, with a few points per edge and some wobble.
    fn hand_drawn(corners: &[Vector2], closed: bool) -> Vec<Vector2> {
        let mut edges = corners.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>();
        if closed {
            edges.push((corners[corners.len() - 1], corners[0]));
        }
        let mut points = Vec::new();
        for (a, b) in edges {
            for step in 0..20 {
                let t = step as f64 / 20.0;
                points.push(a + (b - a) * t + wobble(points.len(), 1.5));
            }
        }
        if !closed {
            points.push(corners[corners.len() - 1]);
        }
        points
    }

    fn ellipse_points(center: Vector2, radii: Vector2, angle: f64, sweep: f64) -> Vec<Vector2> {
        let rotation = DAffine2::from_angle_translation(angle, center);
        (0..80)
            .map(|i| {
                let t = sweep * i as f64 / 80.0;
                rotation.transform_point2(Vector2::new(t.cos(), t.sin()) * radii) + wobble(i, 1.5)
            })
            .collect()
    }

    #[test]
    fn lines_snap_to_the_axes() {
        let points = hand_drawn(&[Vector2::new(0.0, 0.0), Vector2::new(200.0, 8.0)], false);
        let Some(Shape::Line(line)) = recognize(&points) else {
            panic!("no line");
        };
        assert!((line.end.y - line.start.y).abs() < 1e-9, "{line:?}");

        // A clearly slanted line stays slanted.
        let points = hand_drawn(&[Vector2::new(0.0, 0.0), Vector2::new(200.0, 100.0)], false);
        let Some(Shape::Line(line)) = recognize(&points) else {
            panic!("no line");
        };
        assert!((line.end - Vector2::new(200.0, 100.0)).length() < 5.0);
    }

    #[test]
    fn open_corners_become_polylines() {
        // An angle: two rays from a common vertex.
        let corners = [
            Vector2::new(200.0, 0.0),
            Vector2::new(0.0, 100.0),
            Vector2::new(200.0, 200.0),
        ];
        let Some(Shape::Polyline(polyline)) = recognize(&hand_drawn(&corners, false)) else {
            panic!("no polyline");
        };
        assert_eq!(polyline.path.len(), 2);
        assert!((polyline.path[0] - corners[1]).length() < 10.0);
    }

    #[test]
    fn circles_and_ellipses() {
        let circle = ellipse_points(
            Vector2::new(100.0, 100.0),
            Vector2::new(80.0, 76.0),
            0.3,
            TAU,
        );
        let Some(Shape::Ellipse(ellipse)) = recognize(&circle) else {
            panic!("no circle");
        };
        assert_eq!(ellipse.radii.x, ellipse.radii.y);
        assert!((ellipse.radii.x - 78.0).abs() < 5.0, "{ellipse:?}");

        let points = ellipse_points(Vector2::new(0.0, 0.0), Vector2::new(120.0, 50.0), 0.5, TAU);
        let Some(Shape::Ellipse(ellipse)) = recognize(&points) else {
            panic!("no ellipse");
        };
        assert!(
            (ellipse.radii.max_element() - 120.0).abs() < 8.0,
            "{ellipse:?}"
        );
        assert!(
            (ellipse.radii.min_element() - 50.0).abs() < 8.0,
            "{ellipse:?}"
        );

        // A stroke that is not quite closed still counts.
        let almost = ellipse_points(
            Vector2::new(0.0, 0.0),
            Vector2::new(90.0, 90.0),
            0.0,
            TAU * 0.93,
        );
        assert!(matches!(recognize(&almost), Some(Shape::Ellipse(_))));
    }

    #[test]
    fn triangles_and_rectangles() {
        let triangle = [
            Vector2::new(0.0, 200.0),
            Vector2::new(100.0, 0.0),
            Vector2::new(220.0, 190.0),
        ];
        let Some(Shape::Polygon(polygon)) = recognize(&hand_drawn(&triangle, true)) else {
            panic!("no triangle");
        };
        assert_eq!(polygon.path.len(), 2);

        // A slightly crooked rectangle is straightened.
        let rectangle = [
            Vector2::new(0.0, 0.0),
            Vector2::new(300.0, 6.0),
            Vector2::new(296.0, 150.0),
            Vector2::new(-3.0, 147.0),
        ];
        let Some(Shape::Rectangle(rect)) = recognize(&hand_drawn(&rectangle, true)) else {
            panic!("no rectangle");
        };
        assert!((rect.cuboid.half_extents.x - 150.0).abs() < 8.0, "{rect:?}");
        assert!((rect.cuboid.half_extents.y - 75.0).abs() < 8.0, "{rect:?}");
        // Close to horizontal, so it is axis aligned.
        assert!(rect.affine.matrix2.x_axis.y.abs() < 1e-9, "{rect:?}");

        let pentagon = (0..5)
            .map(|i| {
                let a = TAU * i as f64 / 5.0;
                Vector2::new(a.cos(), a.sin()) * 150.0
            })
            .collect::<Vec<_>>();
        let Some(Shape::Polygon(polygon)) = recognize(&hand_drawn(&pentagon, true)) else {
            panic!("no pentagon");
        };
        assert_eq!(polygon.path.len(), 4);
    }

    #[test]
    fn scribbles_are_not_shapes() {
        let scribble = (0..120)
            .map(|i| {
                let t = i as f64 * 0.35;
                Vector2::new(t * 12.0, (t * 3.1).sin() * 40.0 + (t * 1.3).cos() * 25.0)
            })
            .collect::<Vec<_>>();
        assert!(recognize(&scribble).is_none());
    }
}
