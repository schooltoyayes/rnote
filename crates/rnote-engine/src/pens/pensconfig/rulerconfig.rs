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
    /// Whether the tick marks show millimeters and centimeters of the document, with zero at
    /// the anchor. Otherwise they are spaced `tick_spacing` apart on screen. Persisted.
    pub metric_scale: bool,
    /// The stroke that is currently drawn along the ruler, to show its length. In-session only.
    #[serde(skip)]
    pub measurement: Option<RulerMeasurement>,
    /// Which kind of ruler is shown. Persisted.
    pub kind: RulerKind,
    /// Size of the set square and the protractor in surface pixels: half the long edge of the
    /// set square, the radius of the protractor. Persisted.
    pub tool_size: f64,
    /// Angles of the two protractor arms in degrees, from the base line towards the arc.
    /// In-session only.
    #[serde(skip)]
    pub protractor_arms: [f64; 2],
}

/// The kinds of rulers.
///
/// All of them are placed with `anchor` and `angle`: the ruler's centerline, the long edge of the
/// set square and the base line of the protractor run along `direction()` through the anchor.
/// The set square and the protractor lie on the side opposite to `normal()`, with the anchor in
/// the middle of their long edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "ruler_kind")]
pub enum RulerKind {
    /// A straight edge spanning the viewport.
    #[default]
    #[serde(rename = "ruler")]
    Ruler,
    /// A right isosceles triangle with a centimeter scale on its long edge and a degree scale
    /// around its middle, like a "Geodreieck".
    #[serde(rename = "set_square")]
    SetSquare,
    /// A half circle with a degree scale and two arms to measure angles.
    #[serde(rename = "protractor")]
    Protractor,
}

/// An edge of the ruler that strokes snap to, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapTarget {
    /// A straight edge through `point` along the unit vector `dir`. `outward` points away from
    /// the ruler body.
    Line {
        point: Vector2,
        dir: Vector2,
        outward: Vector2,
    },
    /// The arc of the protractor.
    Arc { center: Vector2, radius: f64 },
}

impl SnapTarget {
    /// The closest point on the target.
    pub fn project(&self, pos: Vector2) -> Vector2 {
        match *self {
            SnapTarget::Line { point, dir, .. } => point + (pos - point).dot(dir) * dir,
            SnapTarget::Arc { center, radius } => {
                center + (pos - center).try_normalize().unwrap_or(Vector2::X) * radius
            }
        }
    }

    fn distance(&self, pos: Vector2) -> f64 {
        (self.project(pos) - pos).length()
    }
}

/// A stroke drawn along the ruler, in document coordinates.
#[derive(Clone, Copy, Debug)]
pub struct RulerMeasurement {
    pub start: Vector2,
    pub end: Vector2,
    /// The edge the stroke is drawn along, in window coordinates.
    pub target: SnapTarget,
}

/// A place where strokes can snap to the ruler.
struct SnapCandidate {
    target: SnapTarget,
    /// Along the target, the range where it can be snapped to: for lines the distance from the
    /// target's point along its direction, for arcs the distance from the base line towards the
    /// arc. `None` if it is unlimited.
    range: Option<(f64, f64)>,
    /// Whether the target can be snapped to from inside the ruler body. Otherwise the body is
    /// dragged there.
    inside_body: bool,
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
            metric_scale: true,
            measurement: None,
            kind: RulerKind::default(),
            tool_size: Self::TOOL_SIZE_DEFAULT,
            protractor_arms: Self::PROTRACTOR_ARMS_DEFAULT,
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
    /// Default / range for `tool_size`, in surface pixels.
    pub const TOOL_SIZE_DEFAULT: f64 = 220.0;
    pub const TOOL_SIZE_MIN: f64 = 100.0;
    pub const TOOL_SIZE_MAX: f64 = 500.0;
    pub const PROTRACTOR_ARMS_DEFAULT: [f64; 2] = [0.0, 60.0];
    /// How far the protractor arms reach past the arc, in surface pixels.
    pub const PROTRACTOR_ARM_EXTENSION_PX: f64 = 48.0;
    /// Distance of the arm handles from the arc, in surface pixels.
    pub const PROTRACTOR_HANDLE_OFFSET_PX: f64 = 30.0;
    /// Radius of the arm handles, in surface pixels.
    pub const PROTRACTOR_HANDLE_RADIUS_PX: f64 = 12.0;
    /// Radius around the arm handles in which touch input grabs them, in surface pixels.
    pub const PROTRACTOR_TOUCH_HANDLE_RADIUS_PX: f64 = 30.0;
    /// Distance from the arms in which touch input grabs them, in surface pixels.
    pub const PROTRACTOR_TOUCH_ARM_DISTANCE_PX: f64 = 18.0;
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

    /// Background behind the length shown while drawing along the ruler.
    pub fn measurement_background_color(dark_mode: bool) -> piet::Color {
        if dark_mode {
            piet::Color::rgba8(40, 40, 40, 230)
        } else {
            piet::Color::rgba8(255, 255, 255, 230)
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

    /// Unit vector from the long edge of the set square and the base line of the protractor
    /// towards their body.
    fn up(&self) -> Vector2 {
        -self.normal()
    }

    /// The corners of the set square in window coordinates: the ends of the long edge, then the
    /// right angle.
    pub fn set_square_corners(&self) -> [Vector2; 3] {
        let (dir, up, size) = (self.direction(), self.up(), self.tool_size);
        [
            self.anchor - size * dir,
            self.anchor + size * dir,
            self.anchor + size * up,
        ]
    }

    /// Unit vector along the protractor arm with the given angle in degrees.
    pub fn protractor_arm_direction(&self, arm_deg: f64) -> Vector2 {
        let arm = arm_deg.to_radians();
        arm.cos() * self.direction() + arm.sin() * self.up()
    }

    /// Where the handle of a protractor arm is, in window coordinates.
    pub fn protractor_handle_pos(&self, arm: usize) -> Vector2 {
        self.anchor
            + (self.tool_size + Self::PROTRACTOR_HANDLE_OFFSET_PX)
                * self.protractor_arm_direction(self.protractor_arms[arm])
    }

    /// The protractor arm at `window_pos` for touch input, which is less precise than a pen or a
    /// mouse: its handle, or the arm itself away from the center. With a finger the arms can't
    /// be drawn along, so they can be grabbed anywhere.
    pub fn hit_protractor_arm_touch_window(&self, window_pos: Vector2) -> Option<usize> {
        if !self.visible || self.kind != RulerKind::Protractor {
            return None;
        }
        let arm_len = self.tool_size + Self::PROTRACTOR_ARM_EXTENSION_PX;
        (0..self.protractor_arms.len())
            .filter_map(|arm| {
                let handle_dist = (self.protractor_handle_pos(arm) - window_pos).length();
                if handle_dist <= Self::PROTRACTOR_TOUCH_HANDLE_RADIUS_PX {
                    return Some((arm, handle_dist));
                }
                let dir = self.protractor_arm_direction(self.protractor_arms[arm]);
                let rel = window_pos - self.anchor;
                let along = rel.dot(dir);
                let dist = (rel - along * dir).length();
                (along >= self.tool_size * 0.25
                    && along <= arm_len
                    && dist <= Self::PROTRACTOR_TOUCH_ARM_DISTANCE_PX)
                    .then_some((arm, dist))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(arm, _)| arm)
    }

    /// The protractor arm whose handle is at `window_pos`, if any.
    pub fn hit_protractor_handle_window(&self, window_pos: Vector2) -> Option<usize> {
        if !self.visible || self.kind != RulerKind::Protractor {
            return None;
        }
        (0..self.protractor_arms.len())
            .map(|arm| (arm, (self.protractor_handle_pos(arm) - window_pos).length()))
            .filter(|(_, dist)| *dist <= Self::PROTRACTOR_HANDLE_RADIUS_PX * 1.5)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(arm, _)| arm)
    }

    /// Turn a protractor arm towards `window_pos`, limited to the half circle.
    pub fn set_protractor_arm_towards(&mut self, arm: usize, window_pos: Vector2) {
        let rel = window_pos - self.anchor;
        let deg = rel
            .dot(self.up())
            .atan2(rel.dot(self.direction()))
            .to_degrees();
        // Below the base line the arm stays at the closer end of the half circle.
        let deg = if deg < -90.0 {
            180.0
        } else {
            deg.clamp(0.0, 180.0)
        };
        self.protractor_arms[arm] = deg;
    }

    /// The angle between the protractor arms in degrees.
    pub fn protractor_angle(&self) -> f64 {
        (self.protractor_arms[1] - self.protractor_arms[0]).abs()
    }

    /// Whether `window_pos` (in window coordinates) lies within the ruler body.
    pub fn hit_body_window(&self, window_pos: Vector2) -> bool {
        if !self.visible {
            return false;
        }
        let rel = window_pos - self.anchor;

        match self.kind {
            RulerKind::Ruler => self.perp_distance(window_pos).abs() <= self.body_half_width,
            RulerKind::SetSquare => {
                // Above the long edge and below both short edges, which run at 45°.
                let along = rel.dot(self.direction());
                let up = rel.dot(self.up());
                up >= 0.0 && up + along.abs() <= self.tool_size
            }
            RulerKind::Protractor => rel.dot(self.up()) >= 0.0 && rel.length() <= self.tool_size,
        }
    }

    /// The distance around the ruler in which strokes snap to it, in surface pixels.
    fn snap_distance_px(&self) -> f64 {
        (self.snap_distance / 100.0) * 2.0 * self.body_half_width
    }

    fn snap_candidates(&self) -> Vec<SnapCandidate> {
        let (dir, normal) = (self.direction(), self.normal());
        let size = self.tool_size;
        let line = |point: Vector2, dir: Vector2, outward: Vector2| SnapTarget::Line {
            point,
            dir,
            outward,
        };

        match self.kind {
            RulerKind::Ruler => Vec::from([1.0, -1.0].map(|side| SnapCandidate {
                target: line(
                    self.anchor + side * self.body_half_width * normal,
                    dir,
                    side * normal,
                ),
                range: None,
                inside_body: false,
            })),
            RulerKind::SetSquare => {
                let [left, right, top] = self.set_square_corners();
                let leg = |from: Vector2| {
                    let dir = (top - from).normalize();
                    // Away from the opposite corner.
                    let outward = Vector2::new(-dir.y, dir.x);
                    let outward = if outward.dot(self.anchor - from) > 0.0 {
                        -outward
                    } else {
                        outward
                    };
                    SnapCandidate {
                        target: line(from, dir, outward),
                        range: Some((0.0, size * std::f64::consts::SQRT_2)),
                        inside_body: false,
                    }
                };
                vec![
                    SnapCandidate {
                        target: line(self.anchor, dir, normal),
                        range: Some((-size, size)),
                        inside_body: false,
                    },
                    leg(left),
                    leg(right),
                ]
            }
            RulerKind::Protractor => {
                let arm_len = size + Self::PROTRACTOR_ARM_EXTENSION_PX;
                let mut candidates = vec![
                    SnapCandidate {
                        target: line(self.anchor, dir, normal),
                        range: Some((-size, size)),
                        inside_body: false,
                    },
                    SnapCandidate {
                        target: SnapTarget::Arc {
                            center: self.anchor,
                            radius: size,
                        },
                        range: Some((0.0, size)),
                        inside_body: false,
                    },
                ];
                // The arms can be drawn along from inside, too, starting at the center.
                candidates.extend(self.protractor_arms.map(|arm_deg| {
                    let arm_dir = self.protractor_arm_direction(arm_deg);
                    SnapCandidate {
                        target: line(self.anchor, arm_dir, Vector2::new(-arm_dir.y, arm_dir.x)),
                        range: Some((0.0, arm_len)),
                        inside_body: true,
                    }
                }));
                candidates
            }
        }
    }

    /// The edge of the ruler that a stroke starting at `pos_doc` snaps to, if it is close
    /// enough to one.
    pub fn snap_target(&self, pos_doc: Vector2, view: RulerView) -> Option<SnapTarget> {
        if !self.visible {
            return None;
        }
        let pos = view.from_doc(pos_doc);
        let snap_dist = self.snap_distance_px();
        let inside_body = self.hit_body_window(pos);

        self.snap_candidates()
            .into_iter()
            .filter(|candidate| !inside_body || candidate.inside_body)
            .filter(|candidate| {
                let Some((min, max)) = candidate.range else {
                    return true;
                };
                let along = match candidate.target {
                    SnapTarget::Line { point, dir, .. } => (pos - point).dot(dir),
                    SnapTarget::Arc { center, .. } => (pos - center).dot(self.up()),
                };
                along >= min - snap_dist && along <= max + snap_dist
            })
            .map(|candidate| (candidate.target, candidate.target.distance(pos)))
            .filter(|(_, dist)| *dist <= snap_dist)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(target, _)| target)
    }

    /// Project `pos_doc` onto the snap target, regardless of distance. Used to keep a stroke
    /// locked to the ruler once it has snapped.
    pub fn project_to_target(
        &self,
        pos_doc: Vector2,
        target: SnapTarget,
        view: RulerView,
    ) -> Vector2 {
        view.to_doc(target.project(view.from_doc(pos_doc)))
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

    /// The angle as it is displayed in the dial, in degrees in `[0°, 360°)`, counterclockwise on
    /// screen.
    pub fn displayed_angle_deg(&self) -> f64 {
        (-self.angle.to_degrees()).rem_euclid(360.0)
    }

    /// Rotate the ruler around its rotation center so that it displays `angle_deg`.
    ///
    /// The rotation center stays where it is, so the ruler turns in place.
    pub fn set_displayed_angle_deg(&mut self, angle_deg: f64) {
        // Of the stored angles for the same direction, the one closest to the current angle.
        let base = -angle_deg.to_radians();
        let new_angle =
            base + ((self.angle - base) / std::f64::consts::TAU).round() * std::f64::consts::TAU;
        let delta = new_angle - self.angle;

        let pivot = self.rotation_center();
        let v = self.anchor - pivot;
        let (sin_a, cos_a) = delta.sin_cos();
        self.anchor = pivot + Vector2::new(v.x * cos_a - v.y * sin_a, v.x * sin_a + v.y * cos_a);
        self.dial_pos = pivot;
        self.angle = new_angle;
    }

    /// The point the ruler turns around: the dial of the ruler, the centroid of the set square
    /// and the center of the protractor's base line.
    pub fn rotation_center(&self) -> Vector2 {
        match self.kind {
            RulerKind::Ruler => self.dial_pos,
            RulerKind::SetSquare => self.anchor + self.up() * self.tool_size / 3.0,
            RulerKind::Protractor => self.anchor,
        }
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

        let target = ruler
            .snap_target(view.to_doc(pos_window + Vector2::new(0.0, 40.0)), view)
            .unwrap();
        let projected =
            view.from_doc(ruler.project_to_target(view.to_doc(pos_window), target, view));

        // Keeps its position along the ruler, and sits exactly one half width off the centerline.
        assert!((projected.x - pos_window.x).abs() < 1e-9);
        assert!((ruler.perp_distance(projected) - ruler.body_half_width).abs() < 1e-9);
    }

    #[test]
    fn set_displayed_angle_turns_the_ruler_around_the_dial() {
        let mut ruler = horizontal_ruler();
        ruler.anchor = Vector2::new(100.0, 150.0);
        ruler.dial_pos = Vector2::new(200.0, 150.0);

        for angle_deg in [37.5, 348.0, 90.0, 270.1, 180.0, 0.0] {
            ruler.set_displayed_angle_deg(angle_deg);

            assert!(
                (ruler.displayed_angle_deg() - angle_deg).abs() < 1e-9,
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
            // The side of the edge the stroke snaps to.
            let at = |offset: f64| {
                ruler
                    .snap_target(view.to_doc(ruler.anchor + Vector2::new(0.0, offset)), view)
                    .map(|target| match target {
                        SnapTarget::Line { outward, .. } => outward.y,
                        SnapTarget::Arc { .. } => panic!("the ruler has no arc"),
                    })
            };

            assert_eq!(at(85.0), Some(1.0), "zoom {total_zoom}");
            assert_eq!(at(-85.0), Some(-1.0), "zoom {total_zoom}");
            assert_eq!(at(95.0), None, "zoom {total_zoom}");
        }
    }

    fn tool(kind: RulerKind) -> RulerConfig {
        RulerConfig {
            kind,
            tool_size: 200.0,
            ..horizontal_ruler()
        }
    }

    #[test]
    fn set_square_body_and_edges() {
        let set_square = tool(RulerKind::SetSquare);
        let view = view(1.0, Vector2::ZERO);
        let anchor = set_square.anchor;
        // The body is above the long edge (the normal points down on screen).
        assert!(set_square.hit_body_window(anchor + Vector2::new(0.0, -100.0)));
        assert!(set_square.hit_body_window(anchor + Vector2::new(90.0, -100.0)));
        assert!(!set_square.hit_body_window(anchor + Vector2::new(110.0, -100.0)));
        assert!(!set_square.hit_body_window(anchor + Vector2::new(0.0, 10.0)));

        // Below the long edge it snaps to it, left of the right short edge to that.
        let snap = |pos: Vector2| {
            set_square
                .snap_target(view.to_doc(pos), view)
                .map(|target| target.project(pos))
        };
        let on_long_edge = snap(anchor + Vector2::new(50.0, 20.0)).unwrap();
        assert!((on_long_edge - (anchor + Vector2::new(50.0, 0.0))).length() < 1e-9);
        let on_short_edge = snap(anchor + Vector2::new(115.0, -100.0)).unwrap();
        // The short edges run at 45°.
        let rel = on_short_edge - anchor;
        assert!((rel.x - rel.y - 200.0).abs() < 1e-9, "{rel:?}");
        // Far away there is nothing to snap to.
        assert_eq!(snap(anchor + Vector2::new(0.0, 200.0)), None);
        assert_eq!(snap(anchor + Vector2::new(400.0, 20.0)), None);
    }

    #[test]
    fn set_square_turns_around_its_centroid() {
        let mut set_square = tool(RulerKind::SetSquare);
        let [a, b, c] = set_square.set_square_corners();
        let centroid = (a + b + c) / 3.0;
        assert!((set_square.rotation_center() - centroid).length() < 1e-9);

        for angle_deg in [30.0, 170.0, 225.0, 359.5, 0.0] {
            set_square.set_displayed_angle_deg(angle_deg);
            assert!(
                (set_square.displayed_angle_deg() - angle_deg).abs() < 1e-9,
                "set {angle_deg}, got {}",
                set_square.displayed_angle_deg()
            );
            assert!((set_square.rotation_center() - centroid).length() < 1e-9);
        }
    }

    #[test]
    fn protractor_arms_can_be_grabbed_by_touch() {
        let mut protractor = tool(RulerKind::Protractor);
        protractor.protractor_arms = [0.0, 90.0];
        let anchor = protractor.anchor;
        let up = Vector2::new(0.0, -1.0);

        // Next to the handle, where a pen would miss it.
        let near_handle = protractor.protractor_handle_pos(1) + Vector2::new(24.0, 0.0);
        assert_eq!(protractor.hit_protractor_handle_window(near_handle), None);
        assert_eq!(
            protractor.hit_protractor_arm_touch_window(near_handle),
            Some(1)
        );
        // Anywhere along the arm, but not at the center where both meet.
        let on_arm = anchor + up * 120.0 + Vector2::new(10.0, 0.0);
        assert_eq!(protractor.hit_protractor_arm_touch_window(on_arm), Some(1));
        assert_eq!(protractor.hit_protractor_arm_touch_window(anchor), None);
        // Away from the arms, the protractor itself is dragged.
        let elsewhere = anchor + protractor.protractor_arm_direction(45.0) * 120.0;
        assert_eq!(protractor.hit_protractor_arm_touch_window(elsewhere), None);
    }

    #[test]
    fn protractor_arms_and_arc() {
        let mut protractor = tool(RulerKind::Protractor);
        let view = view(1.0, Vector2::ZERO);
        let anchor = protractor.anchor;

        // The arm follows the pointer, but not below the base line.
        protractor.set_protractor_arm_towards(1, anchor + Vector2::new(-100.0, -100.0));
        assert!((protractor.protractor_arms[1] - 135.0).abs() < 1e-9);
        protractor.set_protractor_arm_towards(1, anchor + Vector2::new(-100.0, 20.0));
        assert!((protractor.protractor_arms[1] - 180.0).abs() < 1e-9);
        protractor.set_protractor_arm_towards(1, anchor + Vector2::new(100.0, 20.0));
        assert_eq!(protractor.protractor_arms[1], 0.0);
        protractor.protractor_arms = [20.0, 60.0];
        assert!((protractor.protractor_angle() - 40.0).abs() < 1e-9);

        // The handles can be grabbed.
        let handle = protractor.protractor_handle_pos(1);
        assert_eq!(protractor.hit_protractor_handle_window(handle), Some(1));
        assert_eq!(protractor.hit_protractor_handle_window(anchor), None);

        // Inside the body strokes snap to the arms, outside to the arc.
        let arm_dir = protractor.protractor_arm_direction(60.0);
        let near_arm = anchor + arm_dir * 100.0 + Vector2::new(3.0, 0.0);
        assert!(protractor.hit_body_window(near_arm));
        let target = protractor.snap_target(view.to_doc(near_arm), view).unwrap();
        let projected = target.project(near_arm) - anchor;
        assert!((projected.normalize() - arm_dir).length() < 1e-9);

        let near_arc = anchor + Vector2::new(0.0, -215.0);
        assert_eq!(
            protractor.snap_target(view.to_doc(near_arc), view),
            Some(SnapTarget::Arc {
                center: anchor,
                radius: 200.0
            })
        );
        // Away from the arms, the body is dragged instead.
        let inside = anchor + protractor.protractor_arm_direction(140.0) * 100.0;
        assert_eq!(protractor.snap_target(view.to_doc(inside), view), None);
        assert!(protractor.hit_body_window(inside));
    }
}
