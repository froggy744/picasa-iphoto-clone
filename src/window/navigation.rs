fn install_smooth_gallery_scroll(
    scrolled: &gtk::ScrolledWindow,
    gallery: Rc<grid::Gallery>,
    ctrl_zoom: bool,
) {
    // GtkScrolledWindow does not ease normal mouse-wheel detents. Its built-in
    // kinetic-scrolling property is for touchscreen scrolling. Keep native
    // touchpad/surface-pixel scrolling, but turn discrete wheel clicks into a
    // short critically-damped animation of the vertical adjustment.
    const WHEEL_STEP_PX: f64 = 120.0;
    const SPRING: f64 = 240.0;
    const DAMPING: f64 = 31.0;
    const STOP_DISTANCE_PX: f64 = 0.35;
    const STOP_SPEED_PX_S: f64 = 4.0;

    // GtkListView keeps its scroll anchor on device-pixel boundaries. Feeding
    // it fractional adjustment values makes GTK immediately write a rounded
    // value back, fighting the animation. GridView/ordinary content accepts
    // fractional values, so only quantize the Folder ListView path.
    let quantize_to_pixels = scrolled
        .child()
        .and_then(|child| child.downcast::<gtk::ListView>().ok())
        .is_some();

    let adjustment = scrolled.vadjustment();
    let target = Rc::new(Cell::new(adjustment.value()));
    let velocity = Rc::new(Cell::new(0.0_f64));
    let active = Rc::new(Cell::new(false));
    let last_frame_us = Rc::new(Cell::new(0_i64));
    let last_animation_value = Rc::new(Cell::new(f64::NAN));
    // Counts consecutive frames where the spring error does not shrink. GTK's
    // ListView anchor corrections can pin the adjustment between two quantized
    // values, which previously left the animation looping "up and down" until
    // an unrelated click/scroll reset it.
    let stall_frames = Rc::new(Cell::new(0_u32));
    let last_error_abs = Rc::new(Cell::new(f64::INFINITY));
    // A scrollbar drag begins with a button press, unlike mouse-wheel motion.
    // Cancel any old wheel spring immediately so the scrollbar cannot be pulled
    // back toward a stale target. This is especially important for folder
    // GtkListView, where value-change based detection conflicts with GTK's own
    // scroll-anchor corrections.
    {
        let adjustment = adjustment.clone();
        let target = target.clone();
        let velocity = velocity.clone();
        let active = active.clone();
        let last_error_abs = last_error_abs.clone();
        let stall_frames = stall_frames.clone();
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |_, _, _, _| {
            active.set(false);
            velocity.set(0.0);
            target.set(adjustment.value());
            stall_frames.set(0);
            last_error_abs.set(f64::INFINITY);
        });
        scrolled.add_controller(click);
    }

    // Keep the value-change stale-target protection only on the ordinary photo
    // grid. The folder view is a GtkListView; during upward scrolling it
    // performs anchor corrections that look like external adjustment jumps.
    // Cancelling on those corrections breaks/warps the smooth wheel animation.
    // Cancel a pending wheel spring when the adjustment moves by an amount we
    // did not animate to. The sensitivity differs by view:
    //
    // - GridView has no scroll anchor corrections, so any movement we did not
    //   make is external and cancels immediately (2 px tolerance).
    // - GtkListView performs sub-row anchor corrections while scrolling, which
    //   must NOT cancel the spring. Only a jump larger than roughly half a
    //   viewport (a programmatic folder navigation, selection scroll, or model
    //   swap) is treated as external. Without this, a stale wheel target fought
    //   the programmatic scroll and produced a large snap (measured 592 px).
    {
        let target = target.clone();
        let velocity = velocity.clone();
        let active = active.clone();
        let last_animation_value = last_animation_value.clone();
        let stall_frames = stall_frames.clone();
        let last_error_abs = last_error_abs.clone();
        adjustment.connect_value_changed(move |adjustment| {
            let value = adjustment.value();
            if active.get() {
                let animated = last_animation_value.get();
                let threshold = if quantize_to_pixels {
                    (adjustment.page_size() * 0.5).max(200.0)
                } else {
                    2.0
                };
                if animated.is_finite() && (value - animated).abs() <= threshold {
                    return;
                }
                // Folder ListView path: a mid-animation jump larger than the
                // tolerance but under ~2 viewports is an anchor re-correction
                // (row-estimate drift, recycled-row range change). Shift the
                // destination by the same delta so the spring keeps
                // converging on the same visual target instead of being
                // cancelled mid-notch and restarted by the next wheel click.
                // Genuine programmatic navigation (zoom anchor restore, model
                // swap) is far larger and still cancels below.
                if quantize_to_pixels
                    && animated.is_finite()
                    && (value - animated).abs() <= adjustment.page_size() * 2.0
                {
                    target.set(target.get() + (value - animated));
                    last_animation_value.set(value);
                    return;
                }
                active.set(false);
                velocity.set(0.0);
                stall_frames.set(0);
                last_error_abs.set(f64::INFINITY);
            }
            target.set(value);
        });
    }

    {
        let adjustment = adjustment.clone();
        let target = target.clone();
        let velocity = velocity.clone();
        let active = active.clone();
        let last_frame_us = last_frame_us.clone();
        let last_animation_value = last_animation_value.clone();
        let stall_frames = stall_frames.clone();
        let last_error_abs = last_error_abs.clone();
        scrolled.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let previous = last_frame_us.replace(now);

            if !active.get() {
                velocity.set(0.0);
                stall_frames.set(0);
                last_error_abs.set(f64::INFINITY);
                return glib::ControlFlow::Continue;
            }

            if previous <= 0 || now <= previous {
                return glib::ControlFlow::Continue;
            }

            // Frame times are microseconds. Clamp long frames so a temporary
            // stall cannot make the spring jump past its destination.
            let dt = ((now - previous) as f64 / 1_000_000.0).clamp(1.0 / 240.0, 0.033);
            let lower = adjustment.lower();
            let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
            let destination = target.get().clamp(lower, upper);
            target.set(destination);

            let current = adjustment.value().clamp(lower, upper);
            let error = destination - current;
            let mut speed = velocity.get();

            if error.abs() <= STOP_DISTANCE_PX && speed.abs() <= STOP_SPEED_PX_S {
                last_animation_value.set(destination);
                adjustment.set_value(destination);
                velocity.set(0.0);
                active.set(false);
                stall_frames.set(0);
                last_error_abs.set(f64::INFINITY);
                return glib::ControlFlow::Continue;
            }

            // Folder ListView path pins were previously detected here by
            // distance alone; that fired mid-approach (speed ~134 px/s at the
            // 12 px boundary) and truncated every wheel run with an abrupt
            // halt. Pin detection now lives in the stall guard below, which
            // requires the error to actually stop shrinking first.

            // Critically damped spring: smooth acceleration into the movement
            // and smooth deceleration at the target. Clamp overshoot explicitly:
            // GtkListView can quantize/anchor-correct folder scrolling, and a
            // tiny spring overshoot around the destination can otherwise look
            // like the view is stuck bouncing up/down until the next click.
            let acceleration = SPRING * error - DAMPING * speed;
            speed += acceleration * dt;
            let proposed = (current + speed * dt).clamp(lower, upper);
            let current_dist = (destination - current).abs();
            let proposed_dist = (destination - proposed).abs();
            // Crossing the destination in one step, or moving *away* from it,
            // both mean the spring has overshot. The divergence case matters
            // for the folder ListView: GTK can push the adjustment past our
            // target while the spring is still moving in that direction, so the
            // spring keeps accelerating away and the stall guard later snaps a
            // large distance (measured 563 px).
            let sign_flip = current_dist > f64::EPSILON
                && (destination - proposed).signum()
                    != (destination - current).signum();
            let diverging = proposed_dist > current_dist;
            if sign_flip || diverging {
                last_animation_value.set(destination);
                adjustment.set_value(destination);
                velocity.set(0.0);
                active.set(false);
                return glib::ControlFlow::Continue;
            }

            let next = if quantize_to_pixels {
                proposed.round().clamp(lower, upper)
            } else {
                proposed
            };

            // Avoid sending a no-op value back through GtkListView's anchor
            // machinery on every frame. If pixel quantization leaves us within
            // one device pixel of the destination, finish the animation instead
            // of accumulating velocity against an unmoving adjustment.
            let moved = (next - current).abs() > f64::EPSILON;
            if moved {
                last_animation_value.set(next);
                adjustment.set_value(next);
            } else if quantize_to_pixels && error.abs() <= 1.0 {
                last_animation_value.set(destination);
                adjustment.set_value(destination);
                velocity.set(0.0);
                active.set(false);
                stall_frames.set(0);
                last_error_abs.set(f64::INFINITY);
                return glib::ControlFlow::Continue;
            }
            velocity.set(speed);

            // Stall guard: when GTK pins the adjustment (anchor correction, or
            // the scrollable range shrinking as recycled rows are unbound), the
            // absolute error stops shrinking and the spring oscillates without
            // converging. Detect that and finish deterministically instead of
            // leaving the view stuck until the next click/scroll.
            //
            // A pin with only a few pixels of residual error (observed 2-12 px
            // on the folder ListView and ~7 px on the GridView) is confirmed
            // after just two frames, and finishes by *accepting* the pinned
            // value with no write-back: snapping to the destination at that
            // point reads as a small jump at the end of a wheel run. Distance
            // alone must never trigger this - during a normal approach the
            // spring legitimately passes through small errors at high speed -
            // only a value that refuses to move further while close counts.
            let error_abs = error.abs();
            let progressed = error_abs < last_error_abs.get() - 0.05;
            last_error_abs.set(error_abs);
            if progressed {
                stall_frames.set(0);
            } else {
                let stalled = stall_frames.get().saturating_add(1);
                stall_frames.set(stalled);
                let near_pin = error_abs <= 12.0;
                if stalled >= if near_pin { 2 } else { 8 } {
                    if near_pin {
                        last_animation_value.set(current);
                    } else {
                        last_animation_value.set(destination);
                        adjustment.set_value(destination);
                    }
                    velocity.set(0.0);
                    active.set(false);
                    stall_frames.set(0);
                    last_error_abs.set(f64::INFINITY);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let adjustment_for_scroll = adjustment.clone();
    let target_for_scroll = target.clone();
    let velocity_for_scroll = velocity.clone();
    let active_for_scroll = active.clone();
    let last_error_abs_for_scroll = last_error_abs.clone();
    controller.connect_scroll(move |controller, _, dy| {
        if controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK)
        {
            active_for_scroll.set(false);
            velocity_for_scroll.set(0.0);
            target_for_scroll.set(adjustment_for_scroll.value());

            if ctrl_zoom {
                if dy < 0.0 {
                    gallery.wheel_zoom_in();
                } else if dy > 0.0 {
                    gallery.wheel_zoom_out();
                }
                return glib::Propagation::Stop;
            }

            // Albums home has no thumbnail-zoom action. Preserve the normal
            // Ctrl+wheel behavior instead of zooming the hidden photo grid.
            return glib::Propagation::Proceed;
        }

        if dy == 0.0 {
            return glib::Propagation::Proceed;
        }

        match controller.unit() {
            gtk::gdk::ScrollUnit::Wheel => {
                let lower = adjustment_for_scroll.lower();
                let upper = (adjustment_for_scroll.upper()
                    - adjustment_for_scroll.page_size())
                    .max(lower);

                let current = adjustment_for_scroll.value().clamp(lower, upper);
                let delta = dy * WHEEL_STEP_PX;
                let pending = target_for_scroll.get() - current;
                let reversing = active_for_scroll.get()
                    && pending.abs() > STOP_DISTANCE_PX
                    && pending.signum() != delta.signum();

                // Rapid clicks in the same direction accumulate naturally. If
                // the wheel reverses direction, however, do not make the user
                // first consume the old queued destination: reverse NOW from
                // the current viewport and discard the old spring velocity.
                let base = if reversing {
                    velocity_for_scroll.set(0.0);
                    current
                } else if active_for_scroll.get() {
                    target_for_scroll.get()
                } else {
                    current
                };
                let destination = (base + delta).clamp(lower, upper);
                target_for_scroll.set(if quantize_to_pixels {
                    destination.round().clamp(lower, upper)
                } else {
                    destination
                });
                // New input is a fresh convergence attempt; clear the stall
                // history so the first frame cannot be mistaken for a stall.
                last_error_abs_for_scroll.set(f64::INFINITY);
                active_for_scroll.set(true);
                glib::Propagation::Stop
            }
            gtk::gdk::ScrollUnit::Surface => {
                // Precision touchpads already provide pixel deltas. Let GTK
                // consume them directly so native touchpad scrolling is kept.
                active_for_scroll.set(false);
                velocity_for_scroll.set(0.0);
                target_for_scroll.set(adjustment_for_scroll.value());
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        }
    });

    scrolled.add_controller(controller);
}



fn install_folder_smooth_gallery_scroll(
    scrolled: &gtk::ScrolledWindow,
    gallery: Rc<grid::Gallery>,
) {
    // Folder mode is a variable-height GtkListView. An absolute spring target
    // can fight ListView's own anchor corrections, producing the visible
    // rebase/debounce seen while scrolling. Smooth only the *remaining wheel
    // distance* instead: every frame advances from GTK's current authoritative
    // adjustment value, so an anchor correction never leaves a stale target to
    // pull the viewport backwards.
    const WHEEL_STEP_PX: f64 = 120.0;
    const TIME_CONSTANT_S: f64 = 0.085;
    const STOP_REMAINING_PX: f64 = 0.35;

    let adjustment = scrolled.vadjustment();
    let remaining = Rc::new(Cell::new(0.0_f64));
    let active = Rc::new(Cell::new(false));
    let last_frame_us = Rc::new(Cell::new(0_i64));

    // A scrollbar drag or pointer press owns the viewport immediately.
    {
        let remaining = remaining.clone();
        let active = active.clone();
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |_, _, _, _| {
            remaining.set(0.0);
            active.set(false);
        });
        scrolled.add_controller(click);
    }

    {
        let adjustment = adjustment.clone();
        let remaining = remaining.clone();
        let active = active.clone();
        let last_frame_us = last_frame_us.clone();
        scrolled.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let previous = last_frame_us.replace(now);

            if !active.get() {
                return glib::ControlFlow::Continue;
            }
            if previous <= 0 || now <= previous {
                return glib::ControlFlow::Continue;
            }

            let dt = ((now - previous) as f64 / 1_000_000.0).clamp(1.0 / 240.0, 0.033);
            let lower = adjustment.lower();
            let upper = (adjustment.upper() - adjustment.page_size()).max(lower);
            let current = adjustment.value().clamp(lower, upper);
            let left = remaining.get();

            if left.abs() <= STOP_REMAINING_PX {
                remaining.set(0.0);
                active.set(false);
                return glib::ControlFlow::Continue;
            }

            // Exponential ease-out. Crucially this is relative to GTK's current
            // value, not an absolute destination. If ListView re-anchors after
            // this write, the next frame simply continues from that new value.
            let fraction = 1.0 - (-dt / TIME_CONSTANT_S).exp();
            let mut step = left * fraction;
            if step.abs() < 0.75 {
                step = 0.75 * step.signum();
                if step.abs() > left.abs() {
                    step = left;
                }
            }

            let next = (current + step).round().clamp(lower, upper);
            let applied = next - current;
            if applied.abs() <= f64::EPSILON {
                remaining.set(0.0);
                active.set(false);
                return glib::ControlFlow::Continue;
            }

            adjustment.set_value(next);
            remaining.set(left - applied);
            glib::ControlFlow::Continue
        });
    }

    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let adjustment_for_scroll = adjustment.clone();
    let remaining_for_scroll = remaining.clone();
    let active_for_scroll = active.clone();
    controller.connect_scroll(move |controller, _, dy| {
        if controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK)
        {
            remaining_for_scroll.set(0.0);
            active_for_scroll.set(false);
            if dy < 0.0 {
                gallery.zoom_in();
            } else if dy > 0.0 {
                gallery.zoom_out();
            }
            return glib::Propagation::Stop;
        }

        if dy == 0.0 {
            return glib::Propagation::Proceed;
        }

        match controller.unit() {
            gtk::gdk::ScrollUnit::Wheel => {
                let lower = adjustment_for_scroll.lower();
                let upper = (adjustment_for_scroll.upper() - adjustment_for_scroll.page_size())
                    .max(lower);
                let current = adjustment_for_scroll.value().clamp(lower, upper);
                if (dy < 0.0 && current <= lower) || (dy > 0.0 && current >= upper) {
                    remaining_for_scroll.set(0.0);
                    active_for_scroll.set(false);
                    return glib::Propagation::Stop;
                }

                let impulse = dy * WHEEL_STEP_PX;
                let old = remaining_for_scroll.get();
                // Direction reversal should react immediately rather than first
                // consuming momentum queued in the opposite direction.
                let next_remaining = if old != 0.0 && old.signum() != impulse.signum() {
                    impulse
                } else {
                    old + impulse
                };
                remaining_for_scroll.set(next_remaining);
                active_for_scroll.set(true);
                glib::Propagation::Stop
            }
            gtk::gdk::ScrollUnit::Surface => {
                // Precision touchpads already deliver smooth pixel deltas.
                // Leave those native and cancel any stale wheel easing.
                remaining_for_scroll.set(0.0);
                active_for_scroll.set(false);
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        }
    });

    scrolled.add_controller(controller);
}

fn install_gallery_zoom_scroll(scrolled: &gtk::ScrolledWindow, gallery: Rc<grid::Gallery>) {
    // Folder mode deliberately leaves ordinary wheel/touchpad/scrollbar input
    // entirely to GTK. Only Ctrl+wheel is intercepted for thumbnail zoom.
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    controller.connect_scroll(move |controller, _, dy| {
        if !controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK)
        {
            return glib::Propagation::Proceed;
        }

        if dy < 0.0 {
            gallery.zoom_in();
        } else if dy > 0.0 {
            gallery.zoom_out();
        }
        glib::Propagation::Stop
    });
    scrolled.add_controller(controller);
}

fn open_in_folder_should_stop(building: bool, revealed: bool, attempt: u32) -> bool {
    (!building && revealed) || attempt >= 800
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderDestinationPlan {
    Normal,
    ReuseWithoutFolderScroll,
    RefreshWithoutFolderScroll,
}

const SEARCH_DEBOUNCE_MS: u64 = 120;

fn folder_destination_plan(
    exact_photo_target: bool,
    reuse_folder_stream: bool,
) -> FolderDestinationPlan {
    if !exact_photo_target {
        FolderDestinationPlan::Normal
    } else if reuse_folder_stream {
        FolderDestinationPlan::ReuseWithoutFolderScroll
    } else {
        FolderDestinationPlan::RefreshWithoutFolderScroll
    }
}

fn can_reuse_folder_stream_for_destination(
    has_folder_target: bool,
    folder_cache_available: bool,
) -> bool {
    has_folder_target && folder_cache_available
}

fn search_folder_focus_should_stop(
    still_on_target: bool,
    search_is_clear: bool,
    building: bool,
    pending: bool,
    focused: bool,
    attempt: u32,
) -> bool {
    if !still_on_target || !search_is_clear {
        return true;
    }
    if !pending {
        return true;
    }
    if building {
        return attempt >= 1200;
    }
    focused || attempt >= 240
}

fn should_ignore_cleared_search_event(
    suppressed: bool,
    cleared_query: Option<&str>,
    query: &str,
) -> bool {
    suppressed || cleared_query.is_some_and(|cleared| cleared == query)
}

#[cfg(test)]
mod open_in_folder_retry_tests {
    use super::{
        can_reuse_folder_stream_for_destination, folder_destination_plan,
        open_in_folder_should_stop, search_folder_focus_should_stop, FolderDestinationPlan,
        should_ignore_cleared_search_event, SEARCH_DEBOUNCE_MS,
    };

    #[test]
    fn stops_immediately_after_photo_is_revealed() {
        assert!(open_in_folder_should_stop(false, true, 1));
    }

    #[test]
    fn keeps_retrying_while_stream_is_building_or_photo_is_not_revealed() {
        assert!(!open_in_folder_should_stop(true, false, 1));
        assert!(!open_in_folder_should_stop(false, false, 25));
    }

    #[test]
    fn hard_limit_still_stops_missing_photo_retry_loop() {
        assert!(open_in_folder_should_stop(false, false, 800));
    }

    #[test]
    fn search_debounce_stays_responsive() {
        assert!(SEARCH_DEBOUNCE_MS <= 150);
    }

    #[test]
    fn folder_destination_reuses_cached_full_stream_even_when_search_was_active() {
        assert!(can_reuse_folder_stream_for_destination(true, true));
        assert!(!can_reuse_folder_stream_for_destination(true, false));
        assert!(!can_reuse_folder_stream_for_destination(false, true));
    }

    #[test]
    fn search_folder_focus_waits_until_the_pending_target_is_consumed() {
        assert!(!search_folder_focus_should_stop(true, true, true, true, false, 1));
        assert!(!search_folder_focus_should_stop(true, true, false, true, false, 1));
        assert!(search_folder_focus_should_stop(true, true, false, false, false, 2));
        assert!(search_folder_focus_should_stop(true, true, false, true, true, 2));
    }

    #[test]
    fn search_folder_focus_stops_if_user_moves_on() {
        assert!(search_folder_focus_should_stop(false, true, true, true, false, 1));
        assert!(search_folder_focus_should_stop(true, false, true, true, false, 1));
    }

    #[test]
    fn stale_event_for_the_query_just_cleared_is_ignored_once() {
        assert!(should_ignore_cleared_search_event(true, None, "marianne"));
        assert!(should_ignore_cleared_search_event(false, Some("marianne"), "marianne"));
        assert!(!should_ignore_cleared_search_event(false, Some("marianne"), "maria"));
    }

    fn exact_photo_navigation_never_schedules_generic_folder_scroll() {
        assert_eq!(
            folder_destination_plan(true, true),
            FolderDestinationPlan::ReuseWithoutFolderScroll
        );
        assert_eq!(
            folder_destination_plan(true, false),
            FolderDestinationPlan::RefreshWithoutFolderScroll
        );
        assert_eq!(
            folder_destination_plan(false, true),
            FolderDestinationPlan::Normal
        );
        assert_eq!(
            folder_destination_plan(false, false),
            FolderDestinationPlan::Normal
        );
    }
}
