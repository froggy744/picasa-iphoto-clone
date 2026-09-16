//! Eased wheel scrolling for the gallery scrollers.
//!
//! GTK moves a mouse wheel in discrete jumps (one detent per event) while
//! touchpads already scroll continuously. This module intercepts only wheel
//! events and animates the vertical adjustment toward an accumulated target
//! with a critically damped spring. A critically damped spring leaves rest at
//! zero velocity, accelerates, then decelerates back to rest, which is the
//! ease-in / ease-out curve we want. Touchpad events (`ScrollUnit::Surface`)
//! are left alone so GTK keeps its native kinetic scrolling.
//!
//! Set `PIC_SMOOTH_SCROLL=0` to disable, `PIC_SMOOTH_SCROLL_STEP` (px/detent)
//! and `PIC_SMOOTH_SCROLL_OMEGA` (rad/s) to tune.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk4 as gtk;

/// Pixels scrolled per wheel detent. GTK's own step is `page_size^(2/3)`.
const DEFAULT_STEP: f64 = 140.0;
/// Spring natural frequency in rad/s. Roughly 48 settles in ~165 ms.
const DEFAULT_OMEGA: f64 = 32.0;
/// A longer frame is treated as this so a resume after a stall is not a jump.
const MAX_DT: f64 = 1.0 / 30.0;
/// Below these thresholds the motion is finished.
const REST_DISTANCE: f64 = 0.5;
const REST_VELOCITY: f64 = 1.0;

/// Advance a critically damped spring by `dt` and return `(position, velocity)`.
///
/// This is the closed-form solution, so it is stable and overshoot-free for any
/// `dt`; a naive explicit integrator blows up at low frame rates.
fn spring_step(position: f64, velocity: f64, target: f64, omega: f64, dt: f64) -> (f64, f64) {
    let offset = position - target;
    let slope = velocity + omega * offset;
    let decay = (-omega * dt).exp();
    let next_position = target + (offset + slope * dt) * decay;
    let next_velocity = decay * (slope - omega * (offset + slope * dt));
    (next_position, next_velocity)
}

struct SmoothScroll {
    adjustment: gtk::Adjustment,
    target: Cell<f64>,
    position: Cell<f64>,
    velocity: Cell<f64>,
    /// Last value written by the spring, so external changes can be told apart.
    written: Cell<f64>,
    ticking: Cell<bool>,
    last_frame: Cell<Option<Instant>>,
    step: f64,
    omega: f64,
}

impl SmoothScroll {
    fn new(adjustment: gtk::Adjustment) -> Rc<Self> {
        let value = adjustment.value();
        let step = env_f64("PIC_SMOOTH_SCROLL_STEP", DEFAULT_STEP).max(1.0);
        let omega = env_f64("PIC_SMOOTH_SCROLL_OMEGA", DEFAULT_OMEGA).clamp(1.0, 120.0);
        Rc::new(Self {
            adjustment,
            target: Cell::new(value),
            position: Cell::new(value),
            velocity: Cell::new(0.0),
            written: Cell::new(value),
            ticking: Cell::new(false),
            last_frame: Cell::new(None),
            step,
            omega,
        })
    }

    fn bounds(&self) -> (f64, f64) {
        let lower = self.adjustment.lower();
        let upper = (self.adjustment.upper() - self.adjustment.page_size()).max(lower);
        (lower, upper)
    }

    /// Add one wheel detent (or a fraction of one) to the animation target.
    fn nudge(&self, dy: f64) {
        let (lower, upper) = self.bounds();
        let target = (self.target.get() + dy * self.step).clamp(lower, upper);
        self.target.set(target);
        self.ticking.set(true);
    }

    /// Adopt an adjustment value that was changed by someone other than the
    /// spring (scrollbar drag, keyboard, `scroll_to`).
    fn reanchor(&self) {
        let value = self.adjustment.value();
        self.target.set(value);
        self.position.set(value);
        self.velocity.set(0.0);
        self.written.set(value);
        self.ticking.set(false);
    }

    fn tick(&self) {
        if !self.ticking.get() {
            self.last_frame.set(None);
            return;
        }
        let now = Instant::now();
        let dt = self
            .last_frame
            .get()
            .map(|last| now.saturating_duration_since(last).as_secs_f64())
            .unwrap_or(1.0 / 60.0)
            .clamp(0.0, MAX_DT);
        self.last_frame.set(Some(now));

        let target = self.target.get();
        let (next, velocity) = spring_step(
            self.position.get(),
            self.velocity.get(),
            target,
            self.omega,
            dt,
        );
        let (lower, upper) = self.bounds();
        let next = next.clamp(lower, upper);
        self.velocity.set(velocity);
        self.position.set(next);
        self.written.set(next);
        self.adjustment.set_value(next);

        if (target - next).abs() < REST_DISTANCE && velocity.abs() < REST_VELOCITY {
            self.velocity.set(0.0);
            self.position.set(target);
            self.written.set(target);
            self.adjustment.set_value(target);
            self.ticking.set(false);
            self.last_frame.set(None);
        }
    }
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Install eased wheel scrolling on `scroller`.
pub fn attach(scroller: &gtk::ScrolledWindow) {
    if std::env::var("PIC_SMOOTH_SCROLL").ok().as_deref() == Some("0") {
        return;
    }
    let adjustment = scroller.vadjustment();
    let smooth = SmoothScroll::new(adjustment.clone());

    {
        let smooth = smooth.clone();
        adjustment.connect_value_changed(move |_| {
            if (smooth.adjustment.value() - smooth.written.get()).abs() > REST_DISTANCE {
                smooth.reanchor();
            }
        });
    }

    {
        let smooth = smooth.clone();
        scroller.add_tick_callback(move |_, _| {
            smooth.tick();
            glib::ControlFlow::Continue
        });
    }

    // Bubble phase on purpose: GTK's own capture-phase controller runs first
    // and cancels any in-flight kinetic deceleration. Last-added controllers
    // run first, so this one runs before the zoom controller (added earlier)
    // and before GTK's internal bubble scroll handler.
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let smooth_for_scroll = smooth;
    controller.connect_scroll(move |controller, _dx, dy| {
        let state = controller.current_event_state();
        // Ctrl+wheel is the zoom shortcut and Shift+wheel is GTK's horizontal
        // mapping; leave both to the existing controllers.
        if state
            .intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK)
        {
            return glib::Propagation::Proceed;
        }
        // Touchpads are already continuous; keep GTK's native kinetic scroll.
        if controller.unit() != gtk::gdk::ScrollUnit::Wheel || dy == 0.0 {
            return glib::Propagation::Proceed;
        }
        smooth_for_scroll.nudge(dy);
        glib::Propagation::Stop
    });
    scroller.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_eases_toward_target_without_overshoot() {
        let (mut position, mut velocity) = (0.0, 0.0);
        let target = 120.0;
        let dt = 1.0 / 60.0;
        let mut furthest = position;
        for _ in 0..120 {
            let (next_position, next_velocity) =
                spring_step(position, velocity, target, DEFAULT_OMEGA, dt);
            position = next_position;
            velocity = next_velocity;
            furthest = furthest.max(position);
        }
        assert!(
            furthest <= target + 1e-6,
            "critically damped spring overshot: {furthest}"
        );
        assert!(
            (position - target).abs() < REST_DISTANCE,
            "spring did not settle: {position}"
        );
    }

    #[test]
    fn spring_reaches_target_quickly() {
        let (mut position, mut velocity): (f64, f64) = (0.0, 0.0);
        let target: f64 = 120.0;
        let dt = 1.0 / 60.0;
        let mut frames = 0;
        while (target - position).abs() >= REST_DISTANCE && frames < 60 {
            let (next_position, next_velocity) =
                spring_step(position, velocity, target, DEFAULT_OMEGA, dt);
            position = next_position;
            velocity = next_velocity;
            frames += 1;
        }
        assert!(frames <= 12, "spring was too slow: {frames} frames");
    }

    #[test]
    fn spring_is_stable_across_frame_rates() {
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 0.25] {
            let (mut position, mut velocity) = (0.0, 0.0);
            let target = 120.0;
            for _ in 0..120 {
                let (next_position, next_velocity) =
                    spring_step(position, velocity, target, DEFAULT_OMEGA, dt);
                position = next_position;
                velocity = next_velocity;
                assert!(
                    position.is_finite() && position <= target + 1e-6,
                    "unstable at dt={dt}: {position}"
                );
            }
        }
    }
}
