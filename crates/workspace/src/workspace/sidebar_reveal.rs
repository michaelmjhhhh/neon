//! The sidebar slides open and closed instead of appearing at once.
//!
//! A spring drives how far open the sidebar is. Unlike a timed curve, it
//! keeps its velocity when the sidebar is toggled again mid-way, so the
//! motion turns around from where it is instead of starting over.

use std::time::Instant;

use gpui_kit::{SpringConfig, SpringState, Window};

/// Critically damped, so it never overshoots and the sheet never pushes past
/// its resting edge. It settles in about a third of a second.
const SPRING: SpringConfig = SpringConfig::new(322., 36., 1.);

/// How close to open or closed counts as there, as a share of the way:
/// about half a pixel of the sidebar.
const EPSILON: f32 = 0.002;

pub(super) struct SidebarReveal {
    spring: SpringState,
    /// When the spring last advanced, while it moves.
    moved_at: Option<Instant>,
}

impl SidebarReveal {
    pub(super) fn new(shown: bool) -> Self {
        Self {
            spring: SpringState {
                position: target(shown),
                velocity: 0.,
            },
            moved_at: None,
        }
    }

    /// Advances toward `shown`, asking for another frame until it gets
    /// there. Without `animate` it lands there at once.
    pub(super) fn step(&mut self, shown: bool, animate: bool, window: &mut Window) {
        let target = target(shown);
        let now = Instant::now();
        if animate {
            let elapsed = self
                .moved_at
                .map_or(0., |moved_at| (now - moved_at).as_secs_f32());
            self.spring = SPRING.step(self.spring, target, elapsed);
        }
        if !animate || SPRING.is_settled(self.spring, target, EPSILON) {
            self.spring = SpringState {
                position: target,
                velocity: 0.,
            };
            self.moved_at = None;
        } else {
            self.moved_at = Some(now);
            window.request_animation_frame();
        }
    }

    /// How far open the sidebar is while it slides, from 0 to 1, or `None`
    /// once it rests open or closed.
    pub(super) fn progress(&self) -> Option<f32> {
        self.moved_at.map(|_| self.spring.position.clamp(0., 1.))
    }
}

fn target(shown: bool) -> f32 {
    if shown { 1. } else { 0. }
}
