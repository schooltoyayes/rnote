// Imports
use super::PenBehaviour;
use super::PenStyle;
use super::pensconfig::brushconfig::BrushStyle;
use super::pensconfig::rulerconfig::{RulerMeasurement, RulerView, SnapTarget};
use super::shaperecognition;
use crate::engine::{EngineTask, EngineTaskSender, EngineView, EngineViewMut};
use crate::store::StrokeKey;
use crate::strokes::Stroke;
use crate::strokes::{BrushStroke, ShapeStroke};
use crate::tasks::OneOffTaskHandle;
use crate::{DrawableOnDoc, WidgetFlags};
use p2d::bounding_volume::{Aabb, BoundingVolume};
use p2d::math::Vector2;
use piet::RenderContext;
use rnote_compose::Constraints;
use rnote_compose::Style;
use rnote_compose::builders::buildable::{Buildable, BuilderCreator, BuilderProgress};
use rnote_compose::builders::{
    PenPathBuilderType, PenPathCurvedBuilder, PenPathModeledBuilder, PenPathSimpleBuilder,
};
use rnote_compose::eventresult::{EventPropagation, EventResult};
use rnote_compose::penevent::{PenEvent, PenProgress};
use rnote_compose::penpath::{Element, Segment};
use std::time::{Duration, Instant};

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum BrushState {
    Idle,
    Drawing {
        path_builder: Box<dyn Buildable<Emit = Segment>>,
        current_stroke_key: StrokeKey,
        preview_style: Style,
        /// If the stroke started snapped to the ruler, the edge it locked to.
        /// All subsequent input is projected onto this edge for the duration
        /// of the stroke. `None` means the stroke started unsnapped and will
        /// never snap.
        ruler_snap: Option<SnapTarget>,
        /// Fires when the pen is held still, to recognize a shape in the stroke. `None` if
        /// shape recognition is off or the stroke snapped to the ruler.
        hold_task: Option<OneOffTaskHandle>,
        /// Where the pen was when it last moved noticeably, in document coordinates.
        hold_pos: Vector2,
    },
    /// The stroke was replaced by a recognized shape, waiting for the pen to be lifted.
    Recognized {
        shape_key: StrokeKey,
    },
    DraggingRuler {
        anchor_begin: Vector2,
        dial_pos_begin: Vector2,
        start_pos: Vector2,
    },
    /// Turning an arm of the protractor.
    DraggingProtractorArm {
        arm: usize,
    },
}

#[derive(Debug)]
pub struct Brush {
    state: BrushState,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            state: BrushState::Idle,
        }
    }
}

impl PenBehaviour for Brush {
    fn init(&mut self, _engine_view: &EngineView) -> WidgetFlags {
        WidgetFlags::default()
    }

    fn deinit(&mut self) -> WidgetFlags {
        WidgetFlags::default()
    }

    fn style(&self) -> PenStyle {
        PenStyle::Brush
    }

    fn update_state(&mut self, _engine_view: &mut EngineViewMut) -> WidgetFlags {
        WidgetFlags::default()
    }

    fn handle_event(
        &mut self,
        event: PenEvent,
        now: Instant,
        engine_view: &mut EngineViewMut,
    ) -> (EventResult<PenProgress>, WidgetFlags) {
        let mut widget_flags = WidgetFlags::default();

        let total_zoom = engine_view.camera.total_zoom();
        let ruler_view = RulerView::from_camera(engine_view.camera);
        let event_result = match (&mut self.state, event) {
            (BrushState::Idle, PenEvent::Down { mut element, .. }) => {
                let ruler = &engine_view.config.pens_config.brush_config.ruler_config;
                // Decide once at the start of the stroke whether to snap to
                // the ruler. The decision is locked for the remainder of the
                // stroke (sticky).
                let ruler_snap = ruler.snap_target(element.pos, ruler_view);
                // If the input lands on a protractor handle, turn its arm. Else if it lands on
                // the ruler body where it can't be drawn along, drag the ruler instead of
                // drawing.
                let dragging = if let Some(arm) =
                    ruler.hit_protractor_handle_window(ruler_view.from_doc(element.pos))
                {
                    Some(BrushState::DraggingProtractorArm { arm })
                } else if ruler_snap.is_none() && ruler.hit_body(element.pos, ruler_view) {
                    Some(BrushState::DraggingRuler {
                        anchor_begin: ruler.anchor,
                        dial_pos_begin: ruler.dial_pos,
                        start_pos: element.pos,
                    })
                } else {
                    None
                };
                if let Some(dragging) = dragging {
                    self.state = dragging;
                    widget_flags.redraw = true;
                    return (
                        EventResult {
                            handled: true,
                            propagate: EventPropagation::Stop,
                            progress: PenProgress::InProgress,
                        },
                        widget_flags,
                    );
                }
                if let Some(target) = ruler_snap {
                    element.pos = ruler.project_to_target(element.pos, target, ruler_view);
                }
                if !element.filter_by_bounds(
                    engine_view
                        .document
                        .bounds()
                        .loosened(Self::INPUT_OVERSHOOT),
                ) {
                    #[cfg(feature = "ui")]
                    {
                        if engine_view.config.pens_config.brush_config.style == BrushStyle::Marker {
                            play_marker_sound(engine_view);
                        } else {
                            trigger_brush_sound(engine_view);
                        }
                    }

                    engine_view
                        .config
                        .pens_config
                        .brush_config
                        .new_style_seeds();

                    let preview_style = Self::get_preview_style(&engine_view.as_im());
                    let brushstroke =
                        Stroke::BrushStroke(BrushStroke::new(element, preview_style.clone()));

                    let current_stroke_key = engine_view.store.insert_stroke(
                        brushstroke,
                        Some(
                            engine_view
                                .config
                                .pens_config
                                .brush_config
                                .layer_for_current_options(),
                        ),
                    );

                    engine_view.store.regenerate_rendering_for_stroke(
                        current_stroke_key,
                        engine_view.camera.viewport(),
                        engine_view.camera.image_scale(),
                    );

                    // Show the length of strokes drawn along the ruler.
                    engine_view
                        .config
                        .pens_config
                        .brush_config
                        .ruler_config
                        .measurement = ruler_snap.map(|target| RulerMeasurement {
                        start: element.pos,
                        end: element.pos,
                        target,
                    });

                    // Strokes along the ruler are straight already.
                    let hold_task = (engine_view
                        .config
                        .pens_config
                        .brush_config
                        .shape_recognition
                        && ruler_snap.is_none())
                    .then(|| Self::new_hold_task(engine_view.tasks_tx.clone()));

                    self.state = BrushState::Drawing {
                        path_builder: new_builder(
                            engine_view.config.pens_config.brush_config.builder_type,
                            element,
                            now,
                        ),
                        current_stroke_key,
                        preview_style,
                        ruler_snap,
                        hold_task,
                        hold_pos: element.pos,
                    };

                    EventResult {
                        handled: true,
                        propagate: EventPropagation::Stop,
                        progress: PenProgress::InProgress,
                    }
                } else {
                    EventResult {
                        handled: false,
                        propagate: EventPropagation::Proceed,
                        progress: PenProgress::Idle,
                    }
                }
            }
            (BrushState::Idle, _) => EventResult {
                handled: false,
                propagate: EventPropagation::Proceed,
                progress: PenProgress::Idle,
            },
            (
                BrushState::DraggingRuler {
                    anchor_begin,
                    dial_pos_begin,
                    start_pos,
                },
                PenEvent::Down { element, .. },
            ) => {
                // Drag delta in doc coords -> in scroller pixels (= doc * zoom).
                let delta_scroller = (element.pos - *start_pos) * total_zoom;
                let ruler = &mut engine_view.config.pens_config.brush_config.ruler_config;
                ruler.anchor = *anchor_begin + delta_scroller;
                ruler.dial_pos = *dial_pos_begin + delta_scroller;
                widget_flags.redraw = true;
                EventResult {
                    handled: true,
                    propagate: EventPropagation::Stop,
                    progress: PenProgress::InProgress,
                }
            }
            (BrushState::DraggingProtractorArm { arm }, PenEvent::Down { element, .. }) => {
                engine_view
                    .config
                    .pens_config
                    .brush_config
                    .ruler_config
                    .set_protractor_arm_towards(*arm, ruler_view.from_doc(element.pos));
                widget_flags.redraw = true;
                EventResult {
                    handled: true,
                    propagate: EventPropagation::Stop,
                    progress: PenProgress::InProgress,
                }
            }
            (
                BrushState::DraggingRuler { .. } | BrushState::DraggingProtractorArm { .. },
                PenEvent::Up { .. } | PenEvent::Cancel,
            ) => {
                self.state = BrushState::Idle;
                widget_flags.redraw = true;
                EventResult {
                    handled: true,
                    propagate: EventPropagation::Stop,
                    progress: PenProgress::Finished,
                }
            }
            (BrushState::DraggingRuler { .. } | BrushState::DraggingProtractorArm { .. }, _) => {
                EventResult {
                    handled: false,
                    propagate: EventPropagation::Proceed,
                    progress: PenProgress::InProgress,
                }
            }
            (BrushState::Recognized { .. }, PenEvent::Down { .. }) => EventResult {
                handled: true,
                propagate: EventPropagation::Stop,
                progress: PenProgress::InProgress,
            },
            (BrushState::Recognized { shape_key }, PenEvent::Up { .. } | PenEvent::Cancel) => {
                engine_view.store.update_geometry_for_stroke(*shape_key);
                engine_view.store.regenerate_rendering_for_stroke_threaded(
                    engine_view.tasks_tx.clone(),
                    *shape_key,
                    engine_view.camera.viewport(),
                    engine_view.camera.image_scale(),
                );
                widget_flags |= engine_view
                    .document
                    .resize_autoexpand(engine_view.store, engine_view.camera);

                self.state = BrushState::Idle;

                widget_flags |= engine_view.store.record(Instant::now());
                widget_flags.store_modified = true;

                EventResult {
                    handled: true,
                    propagate: EventPropagation::Stop,
                    progress: PenProgress::Finished,
                }
            }
            (BrushState::Recognized { .. }, _) => EventResult {
                handled: false,
                propagate: EventPropagation::Proceed,
                progress: PenProgress::InProgress,
            },
            (
                BrushState::Drawing {
                    current_stroke_key, ..
                },
                PenEvent::Cancel,
            ) => {
                if let Some(Stroke::BrushStroke(brushstroke)) =
                    engine_view.store.get_stroke_mut(*current_stroke_key)
                {
                    brushstroke.style = engine_view
                        .config
                        .pens_config
                        .brush_config
                        .style_for_current_options();
                }

                // Finish up the last stroke
                engine_view
                    .store
                    .update_geometry_for_stroke(*current_stroke_key);
                engine_view.store.regenerate_rendering_for_stroke_threaded(
                    engine_view.tasks_tx.clone(),
                    *current_stroke_key,
                    engine_view.camera.viewport(),
                    engine_view.camera.image_scale(),
                );
                widget_flags |= engine_view
                    .document
                    .resize_autoexpand(engine_view.store, engine_view.camera);

                engine_view
                    .config
                    .pens_config
                    .brush_config
                    .ruler_config
                    .measurement = None;
                self.state = BrushState::Idle;

                widget_flags |= engine_view.store.record(Instant::now());
                widget_flags.store_modified = true;

                EventResult {
                    handled: true,
                    propagate: EventPropagation::Stop,
                    progress: PenProgress::Finished,
                }
            }
            (
                BrushState::Drawing {
                    path_builder,
                    current_stroke_key,
                    ruler_snap,
                    hold_task,
                    hold_pos,
                    ..
                },
                mut pen_event,
            ) => {
                // Wait for the pen to be held still again whenever it moves noticeably.
                if let (Some(task), PenEvent::Down { element, .. }) = (hold_task, &pen_event)
                    && (element.pos - *hold_pos).length() > Self::HOLD_TOLERANCE_PX / total_zoom
                {
                    *hold_pos = element.pos;
                    if task.reset_timeout().is_err() {
                        // It fired already, without a shape to recognize.
                        *task = Self::new_hold_task(engine_view.tasks_tx.clone());
                    }
                }
                // If the stroke started snapped, every subsequent point is
                // projected onto the same edge — the stroke stays on the
                // ruler until release, regardless of how far the input moves
                // perpendicular to it.
                if let Some(target) = *ruler_snap {
                    match &mut pen_event {
                        PenEvent::Down { element, .. } | PenEvent::Up { element, .. } => {
                            let ruler_config =
                                &mut engine_view.config.pens_config.brush_config.ruler_config;
                            element.pos =
                                ruler_config.project_to_target(element.pos, target, ruler_view);
                            if let Some(measurement) = &mut ruler_config.measurement {
                                measurement.end = element.pos;
                            }
                        }
                        _ => {}
                    }
                }
                let builder_result =
                    path_builder.handle_event(pen_event, now, Constraints::default());
                let handled = builder_result.handled;
                let propagate = builder_result.propagate;

                let progress = match builder_result.progress {
                    BuilderProgress::InProgress => {
                        #[cfg(feature = "ui")]
                        {
                            if engine_view.config.pens_config.brush_config.style
                                != BrushStyle::Marker
                            {
                                trigger_brush_sound(engine_view);
                            }
                        }

                        PenProgress::InProgress
                    }
                    BuilderProgress::EmitContinue(segments) => {
                        #[cfg(feature = "ui")]
                        {
                            if engine_view.config.pens_config.brush_config.style
                                != BrushStyle::Marker
                            {
                                trigger_brush_sound(engine_view);
                            }
                        }

                        let n_segments = segments.len();

                        if n_segments != 0 {
                            if let Some(Stroke::BrushStroke(brushstroke)) =
                                engine_view.store.get_stroke_mut(*current_stroke_key)
                            {
                                brushstroke.extend_w_segments(segments);
                                widget_flags.store_modified = true;
                            }

                            engine_view.store.append_rendering_last_segments(
                                engine_view.tasks_tx.clone(),
                                *current_stroke_key,
                                n_segments,
                                engine_view.camera.viewport(),
                                engine_view.camera.image_scale(),
                            );
                        }

                        PenProgress::InProgress
                    }
                    BuilderProgress::Finished(segments) => {
                        let n_segments = segments.len();

                        if n_segments != 0 {
                            if let Some(Stroke::BrushStroke(brushstroke)) =
                                engine_view.store.get_stroke_mut(*current_stroke_key)
                            {
                                brushstroke.extend_w_segments(segments);
                                widget_flags.store_modified = true;
                            }

                            engine_view.store.append_rendering_last_segments(
                                engine_view.tasks_tx.clone(),
                                *current_stroke_key,
                                n_segments,
                                engine_view.camera.viewport(),
                                engine_view.camera.image_scale(),
                            );
                        }

                        if let Some(Stroke::BrushStroke(brushstroke)) =
                            engine_view.store.get_stroke_mut(*current_stroke_key)
                        {
                            brushstroke.style = engine_view
                                .config
                                .pens_config
                                .brush_config
                                .style_for_current_options();
                        }

                        // Finish up the last stroke
                        engine_view
                            .store
                            .update_geometry_for_stroke(*current_stroke_key);
                        engine_view.store.regenerate_rendering_for_stroke_threaded(
                            engine_view.tasks_tx.clone(),
                            *current_stroke_key,
                            engine_view.camera.viewport(),
                            engine_view.camera.image_scale(),
                        );
                        widget_flags |= engine_view
                            .document
                            .resize_autoexpand(engine_view.store, engine_view.camera);

                        engine_view
                            .config
                            .pens_config
                            .brush_config
                            .ruler_config
                            .measurement = None;
                        self.state = BrushState::Idle;

                        widget_flags |= engine_view.store.record(Instant::now());
                        widget_flags.store_modified = true;

                        PenProgress::Finished
                    }
                };

                EventResult {
                    handled,
                    propagate,
                    progress,
                }
            }
        };

        (event_result, widget_flags)
    }
}

impl DrawableOnDoc for Brush {
    fn bounds_on_doc(&self, engine_view: &EngineView) -> Option<Aabb> {
        let style = engine_view
            .config
            .pens_config
            .brush_config
            .style_for_current_options();

        match &self.state {
            BrushState::Idle => None,
            BrushState::DraggingRuler { .. }
            | BrushState::DraggingProtractorArm { .. }
            | BrushState::Recognized { .. } => None,
            BrushState::Drawing { path_builder, .. } => {
                path_builder.bounds(&style, engine_view.camera.zoom())
            }
        }
    }

    fn draw_on_doc(
        &self,
        cx: &mut piet_cairo::CairoRenderContext,
        engine_view: &EngineView,
    ) -> anyhow::Result<()> {
        cx.save().map_err(|e| anyhow::anyhow!("{e:?}"))?;

        match &self.state {
            BrushState::Idle
            | BrushState::DraggingRuler { .. }
            | BrushState::DraggingProtractorArm { .. }
            | BrushState::Recognized { .. } => {}
            BrushState::Drawing {
                path_builder,
                preview_style,
                ..
            } => {
                match engine_view.config.pens_config.brush_config.style {
                    BrushStyle::Marker => {
                        // Don't draw the marker, as the pen would render on top of other strokes, while the stroke itself would render underneath them.
                    }
                    BrushStyle::Solid | BrushStyle::Textured => {
                        path_builder.draw_styled(
                            cx,
                            preview_style,
                            engine_view.camera.total_zoom(),
                        );
                    }
                }
            }
        }

        cx.restore().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(())
    }
}

impl Brush {
    const INPUT_OVERSHOOT: f64 = 30.0;
    /// How long the pen has to be held still to recognize a shape.
    const HOLD_DURATION: Duration = Duration::from_millis(500);
    /// Movements below this don't count when holding the pen still, in surface pixels.
    const HOLD_TOLERANCE_PX: f64 = 4.0;

    fn new_hold_task(tasks_tx: EngineTaskSender) -> OneOffTaskHandle {
        OneOffTaskHandle::new(
            move || tasks_tx.send(EngineTask::BrushHold),
            Self::HOLD_DURATION,
        )
    }

    /// The pen was held still while drawing: recognize a shape in the stroke drawn so far, and
    /// if there is one, replace the stroke with it.
    pub(crate) fn handle_hold(&mut self, engine_view: &mut EngineViewMut) -> WidgetFlags {
        let mut widget_flags = WidgetFlags::default();
        let BrushState::Drawing {
            current_stroke_key,
            hold_task: Some(_),
            hold_pos,
            ..
        } = &self.state
        else {
            return widget_flags;
        };
        let Some(Stroke::BrushStroke(brushstroke)) =
            engine_view.store.get_stroke_ref(*current_stroke_key)
        else {
            return widget_flags;
        };
        // The builder may not have passed on the last points to the stroke yet, the pen is
        // still where it was held.
        let points = std::iter::once(brushstroke.path.start.pos)
            .chain(brushstroke.path.segments.iter().map(|s| s.end().pos))
            .chain(std::iter::once(*hold_pos))
            .collect::<Vec<Vector2>>();
        let Some(shape) = shaperecognition::recognize(&points) else {
            return widget_flags;
        };

        let brush_config = &engine_view.config.pens_config.brush_config;
        let shapestroke = ShapeStroke::new(shape, brush_config.shape_style_for_current_options());
        let layer = brush_config.layer_for_current_options();
        engine_view.store.remove_stroke(*current_stroke_key);
        let shape_key = engine_view
            .store
            .insert_stroke(Stroke::ShapeStroke(shapestroke), Some(layer));
        engine_view.store.regenerate_rendering_for_stroke(
            shape_key,
            engine_view.camera.viewport(),
            engine_view.camera.image_scale(),
        );
        self.state = BrushState::Recognized { shape_key };

        widget_flags.redraw = true;
        widget_flags.store_modified = true;
        widget_flags
    }

    fn get_preview_style(engine_view: &EngineView) -> Style {
        let mut style = engine_view
            .config
            .pens_config
            .brush_config
            .style_for_current_options();

        if let Some(mut stroke_color) = style.stroke_color() {
            stroke_color.a = 1.0;
            style.set_stroke_color(stroke_color);
        }

        style
    }
}

#[cfg(feature = "ui")]
fn play_marker_sound(engine_view: &mut EngineViewMut) {
    if let Some(audioplayer) = engine_view.audioplayer {
        audioplayer.play_random_marker_sound();
    }
}

#[cfg(feature = "ui")]
fn trigger_brush_sound(engine_view: &mut EngineViewMut) {
    if let Some(audioplayer) = engine_view.audioplayer.as_mut() {
        audioplayer.trigger_random_brush_sound();
    }
}

fn new_builder(
    builder_type: PenPathBuilderType,
    element: Element,
    now: Instant,
) -> Box<dyn Buildable<Emit = Segment>> {
    match builder_type {
        PenPathBuilderType::Simple => Box::new(PenPathSimpleBuilder::start(element, now)),
        PenPathBuilderType::Curved => Box::new(PenPathCurvedBuilder::start(element, now)),
        PenPathBuilderType::Modeled => Box::new(PenPathModeledBuilder::start(element, now)),
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use crate::engine::EngineTask;
    use crate::pens::PenStyle;
    use crate::strokes::Stroke;
    use p2d::math::Vector2;
    use rnote_compose::penevent::PenEvent;
    use rnote_compose::penpath::Element;
    use rnote_compose::shapes::Shape;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    /// Draw a slightly wobbly, almost closed circle. Returns where the pen stopped.
    fn draw_circle(engine: &mut Engine, start: Instant) -> Vector2 {
        let center = Vector2::new(300.0, 300.0);
        let mut pos = center;
        for i in 0..=60u64 {
            let t = std::f64::consts::TAU * 0.97 * i as f64 / 60.0;
            let wobble = Vector2::new((i as f64 * 1.7).sin(), (i as f64 * 2.3).cos()) * 2.0;
            pos = center + Vector2::new(t.cos(), t.sin()) * 100.0 + wobble;
            let _ = engine.handle_pen_event(
                PenEvent::Down {
                    element: Element::new(pos, 0.5),
                    modifier_keys: HashSet::new(),
                },
                None,
                start + Duration::from_millis(i * 10),
            );
        }
        pos
    }

    fn lift_pen(engine: &mut Engine, pos: Vector2, now: Instant) {
        let _ = engine.handle_pen_event(
            PenEvent::Up {
                element: Element::new(pos, 0.5),
                modifier_keys: HashSet::new(),
            },
            None,
            now,
        );
    }

    fn only_stroke(engine: &Engine) -> Stroke {
        let keys = engine.store.keys_sorted_chrono();
        assert_eq!(keys.len(), 1, "expected exactly one stroke");
        engine.store.get_stroke_ref(keys[0]).unwrap().clone()
    }

    #[test]
    fn holding_the_pen_turns_the_stroke_into_a_shape() {
        let mut engine = Engine::default();
        let _ = engine.change_pen_style(PenStyle::Brush);
        let start = Instant::now();

        let pos = draw_circle(&mut engine, start);
        // The pen is held still until the hold task fires.
        let _ = engine.handle_engine_task(EngineTask::BrushHold);
        lift_pen(&mut engine, pos, start + Duration::from_secs(2));

        let Stroke::ShapeStroke(shapestroke) = only_stroke(&engine) else {
            panic!("the stroke was not replaced by a shape");
        };
        let Shape::Ellipse(ellipse) = shapestroke.shape else {
            panic!("not a circle: {:?}", shapestroke.shape);
        };
        assert!((ellipse.radii.x - 100.0).abs() < 8.0, "{ellipse:?}");
        assert_eq!(ellipse.radii.x, ellipse.radii.y);
    }

    #[test]
    fn the_hold_task_fires_after_holding_still() {
        use futures::FutureExt;

        let mut engine = Engine::default();
        let mut tasks_rx = engine.take_engine_tasks_rx().unwrap();
        let _ = engine.change_pen_style(PenStyle::Brush);
        let start = Instant::now();

        let pos = draw_circle(&mut engine, start);
        std::thread::sleep(Duration::from_millis(800));
        // Handle the tasks that arrived in the meantime, like the app does.
        let mut got_hold = false;
        while let Some(Some(task)) = tasks_rx.recv().now_or_never() {
            got_hold |= matches!(task, EngineTask::BrushHold);
            let _ = engine.handle_engine_task(task);
        }
        assert!(got_hold, "the hold task did not fire");
        lift_pen(&mut engine, pos, Instant::now());

        assert!(matches!(only_stroke(&engine), Stroke::ShapeStroke(_)));
    }

    #[test]
    fn without_holding_the_stroke_stays() {
        let mut engine = Engine::default();
        let _ = engine.change_pen_style(PenStyle::Brush);
        let start = Instant::now();

        let pos = draw_circle(&mut engine, start);
        lift_pen(&mut engine, pos, start + Duration::from_secs(1));
        // A hold that arrives too late changes nothing.
        let _ = engine.handle_engine_task(EngineTask::BrushHold);

        assert!(matches!(only_stroke(&engine), Stroke::BrushStroke(_)));
    }
}
