// Imports
use crate::document::format::MeasureUnit;
use crate::pens::pensconfig::rulerconfig::{RulerConfig, RulerKind, RulerView, SnapTarget};
use p2d::bounding_volume::Aabb;
use p2d::math::Vector2;
use piet::{RenderContext, Text, TextLayout, TextLayoutBuilder};
use rnote_compose::color;
use rnote_compose::ext::Vector2Ext;

// Body, edge, tick and text colors are picked at runtime via `RulerConfig`
// helpers so they can adapt to dark / light backgrounds. The red rotation
// indicator stays the same in both modes — it's a high-contrast accent.
const INDICATOR_COLOR: piet::Color = color::GNOME_REDS[2];

/// Font size of the angle text, in surface pixels (constant on-screen).
const ANGLE_TEXT_SIZE_PX: f64 = 14.0;
/// Outer radius of the angle dial, in surface pixels (constant on-screen).
const DIAL_OUTER_RADIUS_PX: f64 = 32.0;
/// Length of dial minor ticks, in surface pixels.
const DIAL_MINOR_TICK_LEN_PX: f64 = 4.0;
/// Length of dial major ticks, in surface pixels.
const DIAL_MAJOR_TICK_LEN_PX: f64 = 7.0;
/// Number of degrees between dial minor ticks.
const DIAL_MINOR_TICK_STEP_DEG: f64 = 6.0;
/// Every Nth dial tick is a major tick.
const DIAL_MAJOR_TICK_EVERY: u32 = 5; // major every 30°
/// Size of each red direction-indicator triangle, in surface pixels.
const INDICATOR_SIZE_PX: f64 = 6.0;
/// Length of medium tick marks, in surface pixels. Drawn every 5th tick.
const EDGE_TICK_MEDIUM_LEN_PX: f64 = 10.0;
/// On the metric scale, ticks closer than this are left out, in surface pixels.
const METRIC_MIN_TICK_SPACING_PX: f64 = 3.0;
/// On the metric scale, labels closer than this are left out, in surface pixels.
const METRIC_MIN_LABEL_SPACING_PX: f64 = 36.0;
/// Font size of the metric scale labels, in surface pixels.
const SCALE_LABEL_SIZE_PX: f64 = 11.0;
/// Font size of the length shown while drawing along the ruler, in surface pixels.
const MEASUREMENT_TEXT_SIZE_PX: f64 = 13.0;
/// Distance between the drawn stroke and its length label, in surface pixels.
const MEASUREMENT_OFFSET_PX: f64 = 10.0;
/// On the degree scale, labels closer than this are left out, in surface pixels.
const DEGREE_MIN_LABEL_SPACING_PX: f64 = 26.0;
/// Radius of the arc between the protractor arms, in surface pixels.
const PROTRACTOR_ANGLE_ARC_RADIUS_PX: f64 = 36.0;
/// Distance of the angle between the protractor arms from their center, in surface pixels.
const PROTRACTOR_ANGLE_TEXT_RADIUS_PX: f64 = 62.0;

/// The steps of the degree scale in degrees for the given on-screen length of a degree: between
/// the ticks, between the medium ticks if there are any, and between the labelled ticks. All of
/// them divide 180°, so both ends of the scale are labelled.
fn degree_steps(px_per_deg: f64) -> (i64, Option<i64>, i64) {
    const TICK_STEPS: [i64; 5] = [1, 5, 10, 30, 90];
    const LABEL_STEPS: [i64; 3] = [10, 30, 90];
    let fits = |step: i64, min_px: f64| step as f64 * px_per_deg >= min_px;

    let tick = TICK_STEPS
        .into_iter()
        .find(|step| fits(*step, METRIC_MIN_TICK_SPACING_PX))
        .unwrap_or(90);
    let label = LABEL_STEPS
        .into_iter()
        .filter(|step| *step >= tick)
        .find(|step| fits(*step, DEGREE_MIN_LABEL_SPACING_PX))
        .unwrap_or(90);
    let medium = TICK_STEPS
        .into_iter()
        .find(|step| *step > tick)
        .filter(|step| *step < label && label % step == 0);
    (tick, medium, label)
}

/// The steps of the metric scale in millimeters for the given on-screen length of a
/// millimeter: between the ticks, between the medium ticks if there are any, and between the
/// labelled ticks, which are always whole centimeters.
fn metric_steps_mm(px_per_mm: f64) -> (i64, Option<i64>, i64) {
    const STEPS_MM: [i64; 9] = [1, 5, 10, 50, 100, 500, 1000, 5000, 10000];
    let fits = |step: i64, min_px: f64| step as f64 * px_per_mm >= min_px;

    let tick = STEPS_MM
        .into_iter()
        .find(|step| fits(*step, METRIC_MIN_TICK_SPACING_PX))
        .unwrap_or(STEPS_MM[STEPS_MM.len() - 1]);
    let label = STEPS_MM
        .into_iter()
        .filter(|step| *step >= tick.max(10))
        .find(|step| fits(*step, METRIC_MIN_LABEL_SPACING_PX))
        .unwrap_or(STEPS_MM[STEPS_MM.len() - 1]);
    let medium = STEPS_MM
        .into_iter()
        .find(|step| *step > tick)
        .filter(|step| *step < label && label % step == 0);
    (tick, medium, label)
}

/// The angle closest to `angle` that keeps text along it upright on screen.
fn upright_angle(angle: f64) -> f64 {
    let angle = angle.rem_euclid(std::f64::consts::TAU);
    if angle > std::f64::consts::FRAC_PI_2 && angle <= 3.0 * std::f64::consts::FRAC_PI_2 {
        angle - std::f64::consts::PI
    } else {
        angle
    }
}

/// Draw text centered at `center`, rotated by `rotation`, with a constant on-screen size.
///
/// With `outward` the text is moved in that direction until it is `MEASUREMENT_OFFSET_PX` away
/// from `center`. With `background` a rounded rectangle is drawn behind it.
#[allow(clippy::too_many_arguments)]
fn draw_label(
    cx: &mut piet_cairo::CairoRenderContext,
    text: String,
    size_px: f64,
    color: piet::Color,
    background: Option<piet::Color>,
    center: Vector2,
    rotation: f64,
    outward: Option<Vector2>,
    total_zoom: f64,
) -> anyhow::Result<()> {
    // The layout is built at a fixed pixel size, then drawn with the zoom undone, so it does not
    // wiggle when the zoom changes.
    let layout = cx
        .text()
        .new_text_layout(text)
        .font(piet::FontFamily::SYSTEM_UI, size_px)
        .text_color(color)
        .build()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let size = layout.size();
    let center = match outward {
        Some(dir) => {
            let half_extent = size.width * 0.5 * dir.x.abs() + size.height * 0.5 * dir.y.abs();
            center + dir * (MEASUREMENT_OFFSET_PX + half_extent) / total_zoom
        }
        None => center,
    };

    cx.save().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    cx.transform(
        kurbo::Affine::translate(center.to_kurbo_vec())
            * kurbo::Affine::rotate(rotation)
            * kurbo::Affine::scale(1.0 / total_zoom),
    );
    if let Some(background) = background {
        let pad = size_px * 0.35;
        let rect = kurbo::Rect::new(
            -size.width * 0.5 - pad,
            -size.height * 0.5 - pad * 0.5,
            size.width * 0.5 + pad,
            size.height * 0.5 + pad * 0.5,
        );
        cx.fill(rect.to_rounded_rect(pad), &background);
    }
    cx.draw_text(
        &layout,
        kurbo::Point::new(-size.width * 0.5, -size.height * 0.5),
    );
    cx.restore().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(())
}

/// Compute the document-space `[min_t, max_t]` parameter range along the ruler
/// direction so that the segment `anchor_doc + t * direction` covers the full
/// viewport (with margin). Returns `None` for a degenerate viewport.
fn viewport_t_range(
    anchor_doc: Vector2,
    direction: Vector2,
    half_w_doc: f64,
    viewport: Aabb,
) -> Option<(f64, f64)> {
    let extent_along = viewport.extents().length() + half_w_doc * 2.0;
    if extent_along < 1e-6 {
        return None;
    }
    let corners = [
        Vector2::new(viewport.mins.x, viewport.mins.y),
        Vector2::new(viewport.maxs.x, viewport.mins.y),
        Vector2::new(viewport.maxs.x, viewport.maxs.y),
        Vector2::new(viewport.mins.x, viewport.maxs.y),
    ];
    let mut min_t = f64::INFINITY;
    let mut max_t = f64::NEG_INFINITY;
    for c in &corners {
        let t = (*c - anchor_doc).dot(direction);
        min_t = min_t.min(t);
        max_t = max_t.max(t);
    }
    let pad = half_w_doc * 4.0;
    Some((min_t - pad, max_t + pad))
}

/// What is needed to draw the ruler at the current zoom.
struct DrawParams {
    view: RulerView,
    total_zoom: f64,
    dark_mode: bool,
    /// The length of a millimeter in document coordinates.
    mm_doc: f64,
}

impl DrawParams {
    /// Convert a length in surface pixels to document coordinates.
    fn px(&self, px: f64) -> f64 {
        px / self.total_zoom
    }
}

/// Draw the ruler: a band across the visible area, a set square or a protractor.
///
/// `visible_doc` is the whole area visible on screen in document coordinates,
/// which in the bounded layouts extends past the document itself. The ruler
/// spans all of it, so it is not cut off outside the page.
///
/// `doc_dpi` is the resolution of the document, used for the metric scale.
pub fn draw_ruler_on_doc(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    visible_doc: Aabb,
    view: RulerView,
    background_color: &rnote_compose::Color,
    doc_dpi: f64,
) -> anyhow::Result<()> {
    if !ruler.visible {
        return Ok(());
    }
    let params = DrawParams {
        view,
        total_zoom: view.total_zoom(),
        dark_mode: RulerConfig::dark_mode_for_background(background_color),
        mm_doc: doc_dpi / MeasureUnit::AMOUNT_MM_IN_INCH,
    };

    cx.save().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    match ruler.kind {
        RulerKind::Ruler => draw_straight_ruler(cx, ruler, visible_doc, &params)?,
        RulerKind::SetSquare => draw_set_square(cx, ruler, &params)?,
        RulerKind::Protractor => draw_protractor(cx, ruler, &params)?,
    }
    draw_measurement(cx, ruler, &params)?;
    cx.restore().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(())
}

/// Fill and outline the ruler body.
fn draw_body(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    body: kurbo::BezPath,
    params: &DrawParams,
) {
    cx.fill(body.clone(), &ruler.body_fill_color(params.dark_mode));
    cx.stroke(
        body,
        &RulerConfig::body_stroke_color(params.dark_mode),
        params.px(1.0),
    );
}

fn draw_straight_ruler(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    visible_doc: Aabb,
    params: &DrawParams,
) -> anyhow::Result<()> {
    let anchor_doc = ruler.anchor_doc(params.view);
    let dir = ruler.direction();
    let normal = ruler.normal();
    let half_w = ruler.body_half_width_doc(params.total_zoom);
    let Some((min_t, max_t)) = viewport_t_range(anchor_doc, dir, half_w, visible_doc) else {
        return Ok(());
    };

    let p_start = anchor_doc + min_t * dir;
    let p_end = anchor_doc + max_t * dir;
    let mut body = kurbo::BezPath::new();
    body.move_to((p_start + half_w * normal).to_kurbo_point());
    body.line_to((p_end + half_w * normal).to_kurbo_point());
    body.line_to((p_end - half_w * normal).to_kurbo_point());
    body.line_to((p_start - half_w * normal).to_kurbo_point());
    body.close_path();
    draw_body(cx, ruler, body, params);

    // Tick marks on both long edges.
    for side in [1.0, -1.0] {
        draw_edge_scale(
            cx,
            ruler,
            anchor_doc + side * half_w * normal,
            dir,
            -side * normal,
            (min_t, max_t),
            params,
        )?;
    }

    if ruler.show_dial {
        draw_angle_dial(
            cx,
            ruler,
            ruler.dial_pos_doc(params.view),
            params.total_zoom,
            params.dark_mode,
        )?;
    }
    Ok(())
}

fn draw_set_square(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    params: &DrawParams,
) -> anyhow::Result<()> {
    let anchor_doc = ruler.anchor_doc(params.view);
    let dir = ruler.direction();
    let up = -ruler.normal();
    let size = params.px(ruler.tool_size);

    let [left, right, top] = ruler
        .set_square_corners()
        .map(|corner| params.view.to_doc(corner));
    let mut body = kurbo::BezPath::new();
    body.move_to(left.to_kurbo_point());
    body.line_to(right.to_kurbo_point());
    body.line_to(top.to_kurbo_point());
    body.close_path();
    draw_body(cx, ruler, body, params);

    // The centimeter scale along the long edge, zero in its middle.
    draw_edge_scale(cx, ruler, anchor_doc, dir, up, (-size, size), params)?;

    // The degree scale around the middle of the long edge, and the perpendicular
    // through it. It has to stay inside the short edges, which are
    // `size / sqrt(2)` away from the middle.
    let scale_radius = size * 0.62;
    draw_degree_scale(cx, ruler, anchor_doc, scale_radius, false, params)?;
    cx.stroke(
        kurbo::Line::new(
            anchor_doc.to_kurbo_point(),
            (anchor_doc + up * (scale_radius - params.px(RulerConfig::TICK_MAJOR_LEN_PX)))
                .to_kurbo_point(),
        ),
        &RulerConfig::tick_color(params.dark_mode),
        params.px(1.0),
    );

    if ruler.show_dial {
        draw_label(
            cx,
            rotation_text(ruler),
            ANGLE_TEXT_SIZE_PX,
            RulerConfig::angle_text_color(params.dark_mode),
            None,
            anchor_doc + up * size * 0.2,
            0.0,
            None,
            params.total_zoom,
        )?;
    }
    Ok(())
}

fn draw_protractor(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    params: &DrawParams,
) -> anyhow::Result<()> {
    let anchor_doc = ruler.anchor_doc(params.view);
    let dir = ruler.direction();
    let radius = params.px(ruler.tool_size);

    // The half circle above the base line. `up` is a quarter turn
    // counterclockwise on screen from `dir`, so the arc sweeps backwards.
    let mut body = kurbo::BezPath::new();
    body.move_to((anchor_doc + radius * dir).to_kurbo_point());
    body.extend(
        kurbo::Arc::new(
            anchor_doc.to_kurbo_point(),
            (radius, radius),
            ruler.angle,
            -std::f64::consts::PI,
            0.0,
        )
        .append_iter(0.1 * params.px(1.0)),
    );
    body.close_path();
    draw_body(cx, ruler, body, params);

    draw_degree_scale(cx, ruler, anchor_doc, radius, true, params)?;

    // The center of the base line, where the arms meet.
    cx.fill(
        kurbo::Circle::new(anchor_doc.to_kurbo_point(), params.px(2.5)),
        &RulerConfig::tick_color(params.dark_mode),
    );

    // The arms with their handles.
    let arm_len = radius + params.px(RulerConfig::PROTRACTOR_ARM_EXTENSION_PX);
    for arm in 0..ruler.protractor_arms.len() {
        let arm_dir = ruler.protractor_arm_direction(ruler.protractor_arms[arm]);
        cx.stroke(
            kurbo::Line::new(
                anchor_doc.to_kurbo_point(),
                (anchor_doc + arm_len * arm_dir).to_kurbo_point(),
            ),
            &INDICATOR_COLOR,
            params.px(1.5),
        );
        let handle = kurbo::Circle::new(
            params
                .view
                .to_doc(ruler.protractor_handle_pos(arm))
                .to_kurbo_point(),
            params.px(RulerConfig::PROTRACTOR_HANDLE_RADIUS_PX),
        );
        cx.fill(handle, &INDICATOR_COLOR.with_alpha(0.35));
        cx.stroke(handle, &INDICATOR_COLOR, params.px(1.5));
    }

    // The angle between the arms, with an arc from one arm to the other.
    let [arm_a, arm_b] = ruler.protractor_arms;
    let angle_arc_radius = params.px(PROTRACTOR_ANGLE_ARC_RADIUS_PX);
    let mut angle_arc = kurbo::BezPath::new();
    angle_arc.move_to(
        (anchor_doc + angle_arc_radius * ruler.protractor_arm_direction(arm_a)).to_kurbo_point(),
    );
    angle_arc.extend(
        kurbo::Arc::new(
            anchor_doc.to_kurbo_point(),
            (angle_arc_radius, angle_arc_radius),
            ruler.angle - arm_a.to_radians(),
            -(arm_b - arm_a).to_radians(),
            0.0,
        )
        .append_iter(0.1 * params.px(1.0)),
    );
    cx.stroke(angle_arc, &INDICATOR_COLOR, params.px(1.5));
    let bisector = ruler.protractor_arm_direction((arm_a + arm_b) * 0.5);
    draw_label(
        cx,
        degrees_text(ruler.protractor_angle()),
        MEASUREMENT_TEXT_SIZE_PX,
        RulerConfig::angle_text_color(params.dark_mode),
        Some(RulerConfig::measurement_background_color(params.dark_mode)),
        anchor_doc + bisector * params.px(PROTRACTOR_ANGLE_TEXT_RADIUS_PX),
        0.0,
        None,
        params.total_zoom,
    )?;
    Ok(())
}

/// Draw tick marks along an edge, which runs through `origin` along `dir`, into the direction
/// `inward`. On the metric scale they are millimeters and centimeters of the document with zero
/// at the origin, otherwise they have a fixed spacing in surface pixels and are independent of
/// the document. `t_range` limits them to a part of the edge.
fn draw_edge_scale(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    origin: Vector2,
    dir: Vector2,
    inward: Vector2,
    (min_t, max_t): (f64, f64),
    params: &DrawParams,
) -> anyhow::Result<()> {
    const MAX_TICKS: i64 = 4096;

    let (tick_step, medium_step, major_step) = if ruler.metric_scale {
        metric_steps_mm(params.mm_doc * params.total_zoom)
    } else {
        (1, Some(5), 10)
    };
    let spacing = if ruler.metric_scale {
        tick_step as f64 * params.mm_doc
    } else {
        params.px(ruler.tick_spacing)
    };
    let i_min = (min_t / spacing).ceil() as i64;
    let i_max = (max_t / spacing).floor() as i64;
    if i_max - i_min > MAX_TICKS {
        return Ok(());
    }
    let tick_color = RulerConfig::tick_color(params.dark_mode);
    let label_inset = params.px(RulerConfig::TICK_MAJOR_LEN_PX + SCALE_LABEL_SIZE_PX * 0.9);

    for i in i_min..=i_max {
        let p = origin + i as f64 * spacing * dir;
        let value = i * tick_step;
        let is_major = value.rem_euclid(major_step) == 0;
        let len = if is_major {
            RulerConfig::TICK_MAJOR_LEN_PX
        } else if medium_step.is_some_and(|step| value.rem_euclid(step) == 0) {
            EDGE_TICK_MEDIUM_LEN_PX
        } else {
            RulerConfig::TICK_MINOR_LEN_PX
        };
        cx.stroke(
            kurbo::Line::new(
                p.to_kurbo_point(),
                (p + params.px(len) * inward).to_kurbo_point(),
            ),
            &tick_color,
            params.px(1.0),
        );
        // Whole centimeters next to the major ticks.
        if is_major && ruler.metric_scale {
            draw_label(
                cx,
                (value.abs() / 10).to_string(),
                SCALE_LABEL_SIZE_PX,
                tick_color,
                None,
                p + label_inset * inward,
                upright_angle(ruler.angle),
                None,
                params.total_zoom,
            )?;
        }
    }
    Ok(())
}

/// Draw a degree scale from 0° to 180° on the half circle around `center`, on the side of the
/// set square and the protractor. The ticks point inward, the degrees are labelled twice, once
/// counting from each end.
///
/// Without `with_ends` the ticks at 0° and 180° are left out, where the half circle meets the
/// long edge of the set square.
fn draw_degree_scale(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    center: Vector2,
    radius: f64,
    with_ends: bool,
    params: &DrawParams,
) -> anyhow::Result<()> {
    let px_per_deg = radius * params.total_zoom * std::f64::consts::PI / 180.0;
    let (tick_step, medium_step, label_step) = degree_steps(px_per_deg);
    let tick_color = RulerConfig::tick_color(params.dark_mode);
    let label_rotation = upright_angle(ruler.angle);
    let outer_row = radius - params.px(RulerConfig::TICK_MAJOR_LEN_PX + SCALE_LABEL_SIZE_PX);
    let inner_row = outer_row - params.px(SCALE_LABEL_SIZE_PX * 1.6);

    for deg in (0..=180).step_by(tick_step as usize) {
        let is_end = deg == 0 || deg == 180;
        if is_end && !with_ends {
            continue;
        }
        let unit = ruler.protractor_arm_direction(deg as f64);
        // The labels at the ends lie on the base line, move them onto the body.
        let label_shift = if is_end {
            params.px(SCALE_LABEL_SIZE_PX * 0.8) * -ruler.normal()
        } else {
            Vector2::ZERO
        };
        let is_labelled = deg % label_step == 0;
        let len = if is_labelled {
            RulerConfig::TICK_MAJOR_LEN_PX
        } else if medium_step.is_some_and(|step| deg % step == 0) {
            EDGE_TICK_MEDIUM_LEN_PX
        } else {
            RulerConfig::TICK_MINOR_LEN_PX
        };
        cx.stroke(
            kurbo::Line::new(
                (center + radius * unit).to_kurbo_point(),
                (center + (radius - params.px(len)) * unit).to_kurbo_point(),
            ),
            &tick_color,
            params.px(1.0),
        );
        if is_labelled {
            for (row, value) in [(outer_row, deg), (inner_row, 180 - deg)] {
                draw_label(
                    cx,
                    value.to_string(),
                    SCALE_LABEL_SIZE_PX,
                    tick_color,
                    None,
                    center + row * unit + label_shift,
                    label_rotation,
                    None,
                    params.total_zoom,
                )?;
            }
        }
    }
    Ok(())
}

/// The length or the angle of the stroke that is drawn along the ruler, next to its end and
/// outside the ruler.
fn draw_measurement(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    params: &DrawParams,
) -> anyhow::Result<()> {
    let Some(measurement) = ruler.measurement else {
        return Ok(());
    };
    let (text, outward) = match measurement.target {
        SnapTarget::Line { outward, .. } => {
            let length_cm = (measurement.end - measurement.start).length() / params.mm_doc / 10.0;
            (format!("{length_cm:.1} cm"), outward)
        }
        SnapTarget::Arc { center, .. } => {
            let center = params.view.to_doc(center);
            let (from, to) = (measurement.start - center, measurement.end - center);
            let angle = from.perp_dot(to).atan2(from.dot(to)).abs().to_degrees();
            (
                degrees_text(angle),
                (measurement.end - center)
                    .try_normalize()
                    .unwrap_or(Vector2::X),
            )
        }
    };
    draw_label(
        cx,
        text,
        MEASUREMENT_TEXT_SIZE_PX,
        RulerConfig::angle_text_color(params.dark_mode),
        Some(RulerConfig::measurement_background_color(params.dark_mode)),
        measurement.end,
        0.0,
        Some(outward),
        params.total_zoom,
    )
}

/// An angle in whole degrees.
fn degrees_text(deg: f64) -> String {
    format!("{:.0}°", deg.round())
}

fn draw_angle_dial(
    cx: &mut piet_cairo::CairoRenderContext,
    ruler: &RulerConfig,
    dial_pos_doc: Vector2,
    total_zoom: f64,
    dark_mode: bool,
) -> anyhow::Result<()> {
    let outer_r = DIAL_OUTER_RADIUS_PX / total_zoom;
    let minor_len = DIAL_MINOR_TICK_LEN_PX / total_zoom;
    let major_len = DIAL_MAJOR_TICK_LEN_PX / total_zoom;
    let tick_w = 1.0 / total_zoom;
    let indicator_size = INDICATOR_SIZE_PX / total_zoom;

    // Dial tick marks: drawn in the world frame (do NOT rotate with the
    // ruler), so they serve as a fixed reference for reading rotation.
    cx.save().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    cx.transform(kurbo::Affine::translate(dial_pos_doc.to_kurbo_vec()));

    let n_ticks = (360.0 / DIAL_MINOR_TICK_STEP_DEG).round() as i32;
    for i in 0..n_ticks {
        let theta = (i as f64) * DIAL_MINOR_TICK_STEP_DEG.to_radians();
        let (sin, cos) = (theta.sin(), theta.cos());
        let is_major = (i as u32).rem_euclid(DIAL_MAJOR_TICK_EVERY) == 0;
        let len = if is_major { major_len } else { minor_len };
        let p_out = kurbo::Point::new(cos * outer_r, sin * outer_r);
        let p_in = kurbo::Point::new(cos * (outer_r - len), sin * (outer_r - len));
        cx.stroke(
            kurbo::Line::new(p_out, p_in),
            &RulerConfig::tick_color(dark_mode),
            tick_w,
        );
    }

    cx.restore().map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // Red direction-indicators: rotate WITH the ruler, so they point at the
    // current angle on the fixed dial.
    cx.save().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    cx.transform(kurbo::Affine::translate(dial_pos_doc.to_kurbo_vec()));
    cx.transform(kurbo::Affine::rotate(ruler.angle));
    for sign in [1.0_f64, -1.0_f64] {
        let tip = kurbo::Point::new(sign * (outer_r - major_len - indicator_size * 0.2), 0.0);
        let base_back = sign * (outer_r - major_len - indicator_size * 1.2);
        let p_left = kurbo::Point::new(base_back, -indicator_size * 0.55);
        let p_right = kurbo::Point::new(base_back, indicator_size * 0.55);
        let mut tri = kurbo::BezPath::new();
        tri.move_to(tip);
        tri.line_to(p_left);
        tri.line_to(p_right);
        tri.close_path();
        cx.fill(tri, &INDICATOR_COLOR);
    }
    cx.restore().map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // Angle text: world-horizontal, centered at the dial position. Ruler is
    // symmetric (θ and θ+π describe the same line), so display in [-90°, 90°).
    //
    // The layout is built once at a FIXED pixel font size so its measured
    // size is zoom-independent (otherwise sub-pixel hinting in the layout
    // wiggles the centered position as the zoom changes). We then undo the
    // camera's zoom inside this transform so the local frame is in surface
    // pixels — the text ends up at a stable, constant on-screen position.
    draw_label(
        cx,
        rotation_text(ruler),
        ANGLE_TEXT_SIZE_PX,
        RulerConfig::angle_text_color(dark_mode),
        None,
        dial_pos_doc,
        0.0,
        None,
        total_zoom,
    )
}

/// The angle the ruler is turned by, as shown in the dial.
fn rotation_text(ruler: &RulerConfig) -> String {
    let normalized_deg = RulerConfig::normalize_angle(ruler.angle).to_degrees();
    // Angles set in the angle row have at most one decimal, while turning the ruler by
    // hand gives arbitrary angles. Show the decimal only for the former. Round first so
    // `-0.3°` doesn't surface as `-0°`; then canonicalize the sign so `format!` doesn't
    // print the negative zero.
    let tenths = (normalized_deg * 10.0).round();
    let rounded = if (normalized_deg * 10.0 - tenths).abs() < 1e-3 {
        tenths / 10.0
    } else {
        normalized_deg.round()
    };
    // Canonicalize: the displayed range is (-90°, 90°], so a rounded -90° is
    // the same orientation as +90° — show +90°. Also avoid printing "-0°".
    let display_value = if rounded == -90.0 {
        90.0
    } else if rounded == 0.0 {
        0.0
    } else {
        rounded
    };
    // Whole degrees without decimals, e.g. set in the angle row: 37.5°
    if display_value.fract() == 0.0 {
        format!("{display_value:.0}°")
    } else {
        format!("{display_value:.1}°")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;
    use crate::pens::pensconfig::rulerconfig::RulerMeasurement;

    /// Draw the ruler onto a throwaway surface, to check the geometry it produces stays valid.
    fn draw_at(
        total_zoom: f64,
        angle: f64,
        metric_scale: bool,
        kind: RulerKind,
    ) -> anyhow::Result<()> {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300)?;
        let cairo_cx = cairo::Context::new(&surface)?;
        let mut piet_cx = piet_cairo::CairoRenderContext::new(&cairo_cx);
        let ruler = RulerConfig {
            visible: true,
            angle,
            anchor: Vector2::new(200.0, 150.0),
            dial_pos: Vector2::new(200.0, 150.0),
            metric_scale,
            kind,
            measurement: Some(RulerMeasurement {
                start: Vector2::new(10.0, 10.0),
                end: Vector2::new(60.0, 10.0),
                target: if kind == RulerKind::Protractor {
                    SnapTarget::Arc {
                        center: Vector2::new(200.0, 150.0),
                        radius: 100.0,
                    }
                } else {
                    SnapTarget::Line {
                        point: Vector2::new(200.0, 150.0),
                        dir: Vector2::X,
                        outward: Vector2::Y,
                    }
                },
            }),
            ..RulerConfig::default()
        };
        let visible_doc = Aabb::new(
            Vector2::ZERO,
            Vector2::new(400.0 / total_zoom, 300.0 / total_zoom),
        );
        // The canvas is offset inside the window, as in the bounded layouts.
        let mut camera = Camera::default().with_zoom(total_zoom);
        camera.set_surface_origin(Vector2::new(120.0, 0.0));

        draw_ruler_on_doc(
            &mut piet_cx,
            &ruler,
            visible_doc,
            RulerView::from_camera(&camera),
            &rnote_compose::Color::WHITE,
            96.0,
        )
    }

    #[test]
    fn draws_across_the_zoom_range() {
        for total_zoom in [Camera::ZOOM_MIN, 1.0, Camera::ZOOM_MAX] {
            for angle in [0.0, 0.42, std::f64::consts::FRAC_PI_2, 2.5] {
                for metric_scale in [true, false] {
                    for kind in [
                        RulerKind::Ruler,
                        RulerKind::SetSquare,
                        RulerKind::Protractor,
                    ] {
                        draw_at(total_zoom, angle, metric_scale, kind).unwrap_or_else(|e| {
                            panic!("drawing {kind:?} failed at zoom {total_zoom}: {e:?}")
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn metric_steps_stay_readable() {
        // Zoomed in: millimeters, half centimeters in between and every centimeter labelled.
        assert_eq!(metric_steps_mm(4.0), (1, Some(5), 10));
        // Around 100% at 96 dpi a millimeter is 3.78 pixels.
        assert_eq!(metric_steps_mm(96.0 / 25.4), (1, Some(5), 10));
        // Zoomed out, the ticks and labels get further apart.
        assert_eq!(metric_steps_mm(1.0), (5, Some(10), 50));
        assert_eq!(metric_steps_mm(0.1), (50, Some(100), 500));

        for px_per_mm in [0.001, 0.05, 0.3, 2.0, 10.0, 100.0] {
            let (tick, medium, label) = metric_steps_mm(px_per_mm);
            assert_eq!(label % tick, 0);
            assert_eq!(label % 10, 0, "labels are whole centimeters");
            if let Some(medium) = medium {
                assert!(tick < medium && medium < label && label % medium == 0);
            }
        }
    }

    #[test]
    fn degree_steps_stay_readable() {
        // A protractor with a radius of 220 pixels: every degree, labels every 10°.
        assert_eq!(
            degree_steps(220.0 * std::f64::consts::PI / 180.0),
            (1, Some(5), 10)
        );
        // The smaller scale of the set square gets labels every 30°.
        assert_eq!(degree_steps(1.5), (5, Some(10), 30));

        for px_per_deg in [0.01, 0.2, 0.5, 1.0, 3.0, 20.0] {
            let (tick, medium, label) = degree_steps(px_per_deg);
            assert_eq!(180 % label, 0, "both ends are labelled");
            assert_eq!(label % tick, 0);
            if let Some(medium) = medium {
                assert!(tick < medium && medium < label && label % medium == 0);
            }
        }
    }

    #[test]
    fn upright_angle_keeps_text_readable() {
        use std::f64::consts::PI;
        for angle in [0.0, 0.5, 1.5, 2.0, PI, 4.0, 5.0, -0.5, -2.0, 7.0] {
            let upright = upright_angle(angle);
            assert!(upright.cos() >= -1e-9, "{angle} -> {upright}");
            // Still along the same line.
            assert!((upright - angle).sin().abs() < 1e-9);
        }
    }
}
