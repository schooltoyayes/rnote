// Imports
use crate::Camera;
use p2d::math::Vector2;
use serde::{Deserialize, Serialize};

/// The mapping between the window coordinates the ruler lives in and document coordinates.
///
/// The ruler stays where it is on screen while the canvas is panned or zoomed, so its position is
/// kept in window coordinates. Those are not the canvas surface coordinates the camera works in:
/// in the bounded layouts the canvas is only as large as the document and is centered in the
/// window, so the two are offset by a margin that changes with the zoom.
#[derive(Clone, Copy, Debug)]
pub struct RulerView {
    surface_origin: Vector2,
    camera_offset: Vector2,
    total_zoom: f64,
}

impl RulerView {
    pub fn from_camera(camera: &Camera) -> Self {
        Self {
            surface_origin: camera.surface_origin(),
            camera_offset: camera.offset(),
            total_zoom: camera.total_zoom(),
        }
    }

    pub fn total_zoom(&self) -> f64 {
        self.total_zoom
    }

    /// Convert a position from window coordinates to document coordinates.
    pub fn to_doc(&self, window_pos: Vector2) -> Vector2 {
        (window_pos - self.surface_origin + self.camera_offset) / self.total_zoom
    }

    /// Convert a position from document coordinates to window coordinates.
    pub fn from_doc(&self, doc_pos: Vector2) -> Vector2 {
        doc_pos * self.total_zoom - self.camera_offset + self.surface_origin
    }
}

/// Configuration and runtime state for the on-canvas ruler.
///
/// The ruler is a translucent straight-edge spanning the viewport. Position is
/// stored in **window coordinates**, so the ruler stays where it is on screen
/// while the canvas is panned or zoomed, like a straight-edge held against the
/// screen. Conversions to document coordinates go through [`RulerView`], which
/// accounts for where the canvas sits inside the window.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename = "ruler_config")]
pub struct RulerConfig {
    /// Whether the ruler is currently shown and active for snapping.
    /// In-session only — not persisted.
    #[serde(skip)]
    pub visible: bool,
    /// Angle of the ruler's long axis in radians (0 = horizontal). In-session only.
    #[serde(skip)]
    pub angle: f64,
    /// A point in window coordinates the ruler centerline passes through.
    /// Defines the origin for tick marks. In-session only.
    #[serde(skip)]
    pub anchor: Vector2,
    /// Where the angle dial / rotation pivot is rendered, in window
    /// coordinates. Always lies on the centerline. Distinct from `anchor` so
    /// the dial can move (e.g., to the finger centroid) without shifting the
    /// tick origin. In-session only.
    #[serde(skip)]
    pub dial_pos: Vector2,
    /// Snap distance beyond the ruler's edge, as a percentage of the ruler's
    /// full width (`2 × BODY_HALF_WIDTH_PX`). Stored as a preference, e.g.
    /// `25.0` means 25%, i.e. half a body-width beyond each edge.
    pub snap_distance: f64,
    /// Whether to snap the ruler's angle to common values (0°, ±45°, ±90°)
    /// during two-finger rotation. Persisted.
    pub angle_snap_enabled: bool,
    /// Whether to render the angle dial in the center of rotation. Persisted.
    pub show_dial: bool,
    /// Half-width of the ruler body in surface pixels. Persisted.
    pub body_half_width: f64,
    /// On-screen spacing between minor long-edge ticks, in surface pixels.
    /// Persisted.
    pub tick_spacing: f64,
    /// Opacity of the ruler body fill, in percent (`0.0` = fully transparent,
    /// `100.0` = fully opaque). Persisted.
    pub body_opacity: f64,
    /// Degrees of ruler rotation per scroll-wheel unit (`dy`). Persisted.
    pub scroll_rotation_step_deg: f64,
}

impl Default for RulerConfig {
    fn default() -> Self {
        Self {
            visible: false,
            angle: 0.0,
            anchor: Vector2::ZERO,
            dial_pos: Vector2::ZERO,
            snap_distance: Self::SNAP_DISTANCE_DEFAULT,
            angle_snap_enabled: true,
            show_dial: true,
            body_half_width: Self::BODY_HALF_WIDTH_DEFAULT,
            tick_spacing: Self::TICK_SPACING_DEFAULT,
            body_opacity: Self::BODY_OPACITY_DEFAULT,
            scroll_rotation_step_deg: Self::SCROLL_ROTATION_STEP_DEG_DEFAULT,
        }
    }
}

impl RulerConfig {
    pub const SNAP_DISTANCE_DEFAULT: f64 = 25.0;
    pub const SNAP_DISTANCE_MIN: f64 = 0.0;
    pub const SNAP_DISTANCE_MAX: f64 = 200.0;
    /// Default / range for `body_half_width`, in surface pixels (constant on-screen size).
    pub const BODY_HALF_WIDTH_DEFAULT: f64 = 60.0;
    pub const BODY_HALF_WIDTH_MIN: f64 = 20.0;
    pub const BODY_HALF_WIDTH_MAX: f64 = 120.0;
    /// Default / range for `tick_spacing`, in surface pixels.
    pub const TICK_SPACING_DEFAULT: f64 = 5.0;
    pub const TICK_SPACING_MIN: f64 = 2.0;
    pub const TICK_SPACING_MAX: f64 = 20.0;
    /// Default / range for `body_opacity`, in percent.
    pub const BODY_OPACITY_DEFAULT: f64 = 35.0;
    pub const BODY_OPACITY_MIN: f64 = 0.0;
    pub const BODY_OPACITY_MAX: f64 = 100.0;
    /// Default / range for `scroll_rotation_step_deg`, in degrees per scroll-wheel unit.
    pub const SCROLL_ROTATION_STEP_DEG_DEFAULT: f64 = 2.0;
    pub const SCROLL_ROTATION_STEP_DEG_MIN: f64 = 0.1;
    pub const SCROLL_ROTATION_STEP_DEG_MAX: f64 = 15.0;
    /// Length of major tick marks in surface pixels.
    pub const TICK_MAJOR_LEN_PX: f64 = 14.0;
    /// Length of minor tick marks in surface pixels.
    pub const TICK_MINOR_LEN_PX: f64 = 7.0;
    /// Half-width of the snap-to-angle window, in degrees, when *approaching*
    /// a target. Within this many degrees of `0`, `±45`, or `±90`, the angle
    /// gets pulled in to the target.
    pub const ANGLE_SNAP_THRESHOLD_DEG: f64 = 3.0;
    /// Half-width of the snap-to-angle window when the ruler is **already
    /// locked** to a target, in degrees. Smaller than the enter threshold so
    /// a single small scroll-wheel click is enough to break out of a snap.
    pub const ANGLE_SNAP_LEAVE_THRESHOLD_DEG: f64 = 0.5;

    /// Unit direction vector along the ruler's long axis.
    pub fn direction(&self) -> Vector2 {
        Vector2::new(self.angle.cos(), self.angle.sin())
    }

    /// Unit normal vector perpendicular to the ruler's long axis (left-hand side).
    pub fn normal(&self) -> Vector2 {
        Vector2::new(-self.angle.sin(), self.angle.cos())
    }

    /// Half-width of the ruler body in document coordinates at the given zoom.
    pub fn body_half_width_doc(&self, total_zoom: f64) -> f64 {
        self.body_half_width / total_zoom
    }

    /// Whether a dark-on-light palette should be used, given the page's
    /// background color. Uses BT.601 relative luminance with a 0.5 threshold:
    /// dark backgrounds (luminance < 0.5) get a light ruler palette and vice
    /// versa.
    pub fn dark_mode_for_background(bg: &rnote_compose::Color) -> bool {
        let luminance = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
        luminance < 0.5
    }

    /// Body fill color computed from `body_opacity` and the dark-mode decision.
    ///
    /// The body's brightness is interpolated against `body_opacity`: at low
    /// opacity we want the body to contrast with the canvas background (so the
    /// ruler is visible at all), and at high opacity we want it to contrast
    /// with the marking colors (so the ticks / text stay readable).
    pub fn body_fill_color(&self, dark_mode: bool) -> piet::Color {
        let opacity = (self.body_opacity / 100.0).clamp(0.0, 1.0);
        let alpha = (opacity * 255.0).round() as u8;
        let (low, high) = if dark_mode {
            // Dark mode: low opacity ≈ near-white (contrast against dark canvas);
            // high opacity ≈ dark gray (contrast against white markings/text).
            (220.0, 60.0)
        } else {
            // Light mode: low opacity ≈ dark gray (contrast against light canvas);
            // high opacity ≈ light gray (contrast against black markings/text).
            (80.0, 200.0)
        };
        let brightness = (low + (high - low) * opacity).round().clamp(0.0, 255.0) as u8;
        piet::Color::rgba8(brightness, brightness, brightness, alpha)
    }

    /// Color used for the body outline.
    pub fn body_stroke_color(dark_mode: bool) -> piet::Color {
        if dark_mode {
            piet::Color::rgba8(255, 255, 255, 220)
        } else {
            piet::Color::rgba8(0, 0, 0, 220)
        }
    }

    /// Color used for the tick marks (long edges + dial ticks).
    pub fn tick_color(dark_mode: bool) -> piet::Color {
        if dark_mode {
            piet::Color::rgba8(255, 255, 255, 240)
        } else {
            piet::Color::rgba8(0, 0, 0, 240)
        }
    }

    /// Color used for the angle text in the dial.
    pub fn angle_text_color(dark_mode: bool) -> piet::Color {
        if dark_mode {
            piet::Color::rgba8(255, 255, 255, 255)
        } else {
            piet::Color::rgba8(0, 0, 0, 255)
        }
    }

    /// Ruler centerline anchor in document coordinates.
    pub fn anchor_doc(&self, view: RulerView) -> Vector2 {
        view.to_doc(self.anchor)
    }

    /// Dial position in document coordinates.
    pub fn dial_pos_doc(&self, view: RulerView) -> Vector2 {
        view.to_doc(self.dial_pos)
    }

    /// Signed distance of a position in window coordinates from the centerline.
    fn perp_distance(&self, window_pos: Vector2) -> f64 {
        (window_pos - self.anchor).dot(self.normal())
    }

    /// Whether `window_pos` (in window coordinates) lies within the ruler body strip.
    pub fn hit_body_window(&self, window_pos: Vector2) -> bool {
        if !self.visible {
            return false;
        }

        self.perp_distance(window_pos).abs() <= self.body_half_width
    }

    /// If `pos_doc` lies within the snap zone of one of the ruler's long
    /// edges, return the sign of the perpendicular (`+1.0` or `-1.0`) that
    /// identifies that edge. `None` means no snap.
    pub fn snap_side(&self, pos_doc: Vector2, view: RulerView) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let half_w = self.body_half_width;
        let snap_dist = (self.snap_distance / 100.0) * 2.0 * half_w;
        let perp = self.perp_distance(view.from_doc(pos_doc));
        if perp.abs() - half_w > snap_dist {
            None
        } else {
            Some(if perp >= 0.0 { 1.0 } else { -1.0 })
        }
    }

    /// Project `pos_doc` onto the long edge identified by `side` (`+1.0` or
    /// `-1.0`), regardless of distance. Used to keep a stroke locked to the
    /// ruler once it has snapped.
    pub fn project_to_edge(&self, pos_doc: Vector2, side: f64, view: RulerView) -> Vector2 {
        let pos_window = view.from_doc(pos_doc);
        let dir = self.direction();
        let normal = self.normal();
        let along = (pos_window - self.anchor).dot(dir);
        let snapped_window = self.anchor + along * dir + side * self.body_half_width * normal;

        view.to_doc(snapped_window)
    }

    /// Normalize an angle (radians) to the displayable principal angle in
    /// `[-π/2, π/2)`, with sign flipped so positive is CCW as the user sees
    /// it on screen (the stored `angle` uses screen-y-down radians where
    /// positive rotation is visually clockwise).
    pub fn normalize_angle(angle_rad: f64) -> f64 {
        let half_pi = std::f64::consts::FRAC_PI_2;
        let pi = std::f64::consts::PI;
        -(((angle_rad + half_pi).rem_euclid(pi)) - half_pi)
    }

    /// The angle as it is displayed in the dial, in degrees, see [`Self::normalize_angle`].
    pub fn displayed_angle_deg(&self) -> f64 {
        Self::normalize_angle(self.angle).to_degrees()
    }

    /// Rotate the ruler around the dial so that it displays `angle_deg`.
    ///
    /// The dial stays where it is, so the ruler turns in place. Of the two stored angles that
    /// describe the same line, the one closer to the current angle is picked so the tick pattern
    /// does not flip around.
    pub fn set_displayed_angle_deg(&mut self, angle_deg: f64) {
        let pi = std::f64::consts::PI;
        let base = -angle_deg.to_radians();
        let new_angle = base + ((self.angle - base) / pi).round() * pi;
        let delta = new_angle - self.angle;

        let v = self.anchor - self.dial_pos;
        let (sin_a, cos_a) = delta.sin_cos();
        self.anchor =
            self.dial_pos + Vector2::new(v.x * cos_a - v.y * sin_a, v.x * sin_a + v.y * cos_a);
        self.angle = new_angle;
    }

    /// Whether `angle_rad` is essentially equal to one of the snap targets
    /// (0°, ±45°, ±90° — modulo π for line symmetry). Used by the
    /// hysteretic snap to know whether to use the "enter" or "leave" window.
    pub fn is_at_snap_target(angle_rad: f64) -> bool {
        const EPS_DEG: f64 = 0.001;
        const TARGETS_DEG: [f64; 5] = [-90.0, -45.0, 0.0, 45.0, 90.0];
        let normalized_deg = Self::normalize_angle(angle_rad).to_degrees();
        TARGETS_DEG
            .iter()
            .any(|t| (normalized_deg - t).abs() < EPS_DEG)
    }

    /// If `angle_rad` is within `threshold_deg` of one of the target angles
    /// (0°, ±45°, ±90°), return the snapped angle in the same
    /// (screen-radian) convention as the input. Otherwise return the input
    /// unchanged.
    pub fn snap_angle_with_threshold(angle_rad: f64, threshold_deg: f64) -> f64 {
        const TARGETS_DEG: [f64; 5] = [-90.0, -45.0, 0.0, 45.0, 90.0];
        let normalized_deg = Self::normalize_angle(angle_rad).to_degrees();
        for t in TARGETS_DEG {
            if (normalized_deg - t).abs() < threshold_deg {
                // The ruler line is symmetric: `base + k*π` for any integer k
                // describes the same physical line direction. We pick the k
                // that keeps the returned value closest to the input — this
                // avoids a π-jump in `ruler.angle` when the user rotates
                // across a snap boundary (which would rotate the anchor
                // around the pivot by π and visibly shift the tick pattern
                // along the ruler's axis).
                let base = -t.to_radians();
                let pi = std::f64::consts::PI;
                let k = ((angle_rad - base) / pi).round();
                return base + k * pi;
            }
        }
        angle_rad
    }

    /// Hysteretic snap: the narrow "leave" window applies only to the target
    /// the ruler is *currently* locked on, so it's easy to escape that one
    /// without making it hard to engage any of the others.
    pub fn snap_angle_hysteretic(raw_angle: f64, prev_angle: f64) -> f64 {
        const TARGETS_DEG: [f64; 5] = [-90.0, -45.0, 0.0, 45.0, 90.0];
        const EPS_DEG: f64 = 0.001;
        let normalized_deg = Self::normalize_angle(raw_angle).to_degrees();
        let prev_normalized_deg = Self::normalize_angle(prev_angle).to_degrees();
        for t in TARGETS_DEG {
            let was_at_this_target = (prev_normalized_deg - t).abs() < EPS_DEG;
            let threshold = if was_at_this_target {
                Self::ANGLE_SNAP_LEAVE_THRESHOLD_DEG
            } else {
                Self::ANGLE_SNAP_THRESHOLD_DEG
            };
            if (normalized_deg - t).abs() < threshold {
                let base = -t.to_radians();
                let pi = std::f64::consts::PI;
                let k = ((raw_angle - base) / pi).round();
                return base + k * pi;
            }
        }
        raw_angle
    }

    /// Back-compat wrapper using the enter-threshold only.
    pub fn snap_angle(angle_rad: f64) -> f64 {
        Self::snap_angle_with_threshold(angle_rad, Self::ANGLE_SNAP_THRESHOLD_DEG)
    }

    /// Whether `pos_doc` (in document coordinates) lies within the ruler body
    /// strip (infinite along the long axis, finite across).
    pub fn hit_body(&self, pos_doc: Vector2, view: RulerView) -> bool {
        self.hit_body_window(view.from_doc(pos_doc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn horizontal_ruler() -> RulerConfig {
        RulerConfig {
            visible: true,
            angle: 0.0,
            anchor: Vector2::new(200.0, 150.0),
            dial_pos: Vector2::new(200.0, 150.0),
            body_half_width: 60.0,
            ..RulerConfig::default()
        }
    }

    /// A view with the canvas offset inside the window, as in the bounded layouts where it is
    /// only as large as the document and centered.
    fn view(total_zoom: f64, surface_origin: Vector2) -> RulerView {
        RulerView {
            surface_origin,
            camera_offset: Vector2::new(40.0, 25.0),
            total_zoom,
        }
    }

    #[test]
    fn window_and_document_conversions_are_inverse() {
        let view = view(2.5, Vector2::new(120.0, 0.0));
        let window_pos = Vector2::new(310.0, 180.0);

        assert!((view.from_doc(view.to_doc(window_pos)) - window_pos).length() < 1e-9);
    }

    #[test]
    fn body_stays_under_the_same_screen_position() {
        let ruler = horizontal_ruler();
        // 50 pixels below the centerline on screen is inside the 60 pixel half width,
        // no matter the zoom or where the canvas sits inside the window.
        let window_pos = ruler.anchor + Vector2::new(0.0, 50.0);

        for total_zoom in [0.2, 1.0, 6.0] {
            for surface_origin in [Vector2::ZERO, Vector2::new(180.0, 40.0)] {
                let view = view(total_zoom, surface_origin);
                assert!(
                    ruler.hit_body(view.to_doc(window_pos), view),
                    "expected a hit at zoom {total_zoom} with origin {surface_origin:?}"
                );
            }
        }
    }

    #[test]
    fn project_to_edge_lands_on_the_edge() {
        let ruler = horizontal_ruler();
        let view = view(2.5, Vector2::new(120.0, 0.0));
        let pos_window = Vector2::new(320.0, 190.0);

        let projected = view.from_doc(ruler.project_to_edge(view.to_doc(pos_window), 1.0, view));

        // Keeps its position along the ruler, and sits exactly one half width off the centerline.
        assert!((projected.x - pos_window.x).abs() < 1e-9);
        assert!((ruler.perp_distance(projected) - ruler.body_half_width).abs() < 1e-9);
    }

    #[test]
    fn set_displayed_angle_turns_the_ruler_around_the_dial() {
        let mut ruler = horizontal_ruler();
        ruler.anchor = Vector2::new(100.0, 150.0);
        ruler.dial_pos = Vector2::new(200.0, 150.0);

        for angle_deg in [37.5, -12.0, 90.0, -89.9, 0.0] {
            ruler.set_displayed_angle_deg(angle_deg);

            assert!(
                (ruler.displayed_angle_deg() - angle_deg).abs() < 1e-9
                    || (angle_deg.abs() - 90.0).abs() < 1e-9,
                "set {angle_deg}, got {}",
                ruler.displayed_angle_deg()
            );
            // The dial stays on the centerline, and the anchor keeps its distance to it.
            assert!(ruler.perp_distance(ruler.dial_pos).abs() < 1e-9);
            assert!(((ruler.anchor - ruler.dial_pos).length() - 100.0).abs() < 1e-9);
        }
        // Back at 0° the ruler is horizontal again with the same tick orientation.
        assert!((ruler.direction() - Vector2::new(1.0, 0.0)).length() < 1e-9);
    }

    #[test]
    fn snap_zone_reaches_past_the_edge_in_screen_pixels() {
        let ruler = horizontal_ruler();
        // The default snap distance is 25% of the full width, so the zone ends half a body
        // half-width past the edge: 60 + 30 = 90 pixels on screen.
        for total_zoom in [0.2, 1.0, 6.0] {
            let view = view(total_zoom, Vector2::new(180.0, 40.0));
            let at = |offset: f64| {
                ruler.snap_side(view.to_doc(ruler.anchor + Vector2::new(0.0, offset)), view)
            };

            assert_eq!(at(85.0), Some(1.0), "zoom {total_zoom}");
            assert_eq!(at(-85.0), Some(-1.0), "zoom {total_zoom}");
            assert_eq!(at(95.0), None, "zoom {total_zoom}");
        }
    }
}
