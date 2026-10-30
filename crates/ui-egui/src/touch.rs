//! Fingers and the Apple Pencil, read before egui turns touches into a pointer.
//!
//! egui-winit emulates the mouse from the first finger, which is what makes the whole interface
//! usable by touch; but it also means a two-finger camera gesture would drag whatever the first
//! finger started on. This module tracks the frame's touches so [`crate::navigation`] can pan and
//! pinch with two fingers while the pointer stays quiet, and so a Pencil stroke never orbits,
//! pans or zooms the view. Palm touches that land while the Pencil is down are not fingers here.

use std::collections::BTreeMap;

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, TouchId, TouchPhase};

/// Whether a touch is a finger, the Pencil, or a palm that landed beside the Pencil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Finger,
    Pencil,
    /// Any touch that began while the Pencil was down; it stays one until it lifts, so a palm
    /// left on the glass does not become a finger the moment the Pencil is raised.
    Palm,
}

/// The touches on the glass this frame, kept between frames.
#[derive(Clone, Debug, Default)]
pub struct TouchState {
    touches: BTreeMap<TouchId, Kind>,
    gesture: bool,
    last_pos: Option<Pos2>,
}

impl TouchState {
    /// The Pencil is touching: extra fingers are palms, and a Pencil drag selects and draws.
    pub fn pencil_down(&self) -> bool {
        self.touches.values().any(|kind| *kind == Kind::Pencil)
    }

    /// Two or more fingers run the camera: orbit, pan and pinch, never the pointer.
    pub fn gesture(&self) -> bool {
        self.gesture
    }

    /// Read the frame's touches and, while a two-finger gesture runs, keep the pointer off them.
    pub fn update(&mut self, raw: &mut RawInput) {
        let mut first_touch = None;
        for event in &raw.events {
            match event {
                Event::Touch { id, phase, pos, force, .. } => {
                    first_touch.get_or_insert(*pos);
                    self.last_pos = Some(*pos);
                    match phase {
                        TouchPhase::Start | TouchPhase::Move => {
                            // The Pencil reports force; an iPad finger reports none, and neither
                            // does a palm. A touch that begins beside the Pencil is a palm.
                            let known = self.touches.get(id).copied();
                            let kind = match (force.is_some(), known) {
                                (true, _) => Kind::Pencil,
                                (false, Some(kind)) => kind,
                                (false, None) => {
                                    if self.pencil_down() {
                                        Kind::Palm
                                    } else {
                                        Kind::Finger
                                    }
                                }
                            };
                            self.touches.insert(*id, kind);
                        }
                        TouchPhase::End | TouchPhase::Cancel => {
                            self.touches.remove(id);
                        }
                    }
                }
                Event::PointerMoved(pos) => self.last_pos = Some(*pos),
                Event::PointerButton { pos, .. } => self.last_pos = Some(*pos),
                _ => {}
            }
        }
        let pencil = self.pencil_down();
        let fingers = self.touches.values().filter(|kind| **kind == Kind::Finger).count();
        let gesture = fingers >= 2 && !pencil;
        let started = gesture && !self.gesture;
        self.gesture = gesture;
        if self.touches.is_empty() {
            self.last_pos = None;
        }
        if !gesture {
            return;
        }
        // The pointer follows the first finger; while the camera has the fingers it must drag
        // nothing, or a pan would also select, move a face or start a sketch.
        raw.events.retain(|event| !matches!(event, Event::PointerMoved(_) | Event::PointerButton { .. }));
        if started && let Some(pos) = first_touch.or(self.last_pos) {
            // A drag that began with the first finger ends where the gesture began.
            raw.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{TouchDeviceId, pos2};

    fn touch(id: u64, phase: TouchPhase, x: f32, force: Option<f32>) -> Event {
        Event::Touch { device_id: TouchDeviceId(0), id: TouchId::from(id), phase, pos: pos2(x, 100.0), force }
    }

    fn frame(events: Vec<Event>) -> RawInput {
        RawInput { events, ..Default::default() }
    }

    fn press(a: &mut TouchState, raw: &mut RawInput) {
        a.update(raw);
    }

    #[test]
    fn one_finger_is_a_pointer_and_no_gesture() {
        let mut s = TouchState::default();
        let mut raw = frame(vec![
            touch(1, TouchPhase::Start, 10.0, None),
            Event::PointerButton { pos: pos2(10.0, 100.0), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        ]);
        press(&mut s, &mut raw);
        assert!(!s.gesture() && !s.pencil_down());
        assert_eq!(raw.events.len(), 2, "the pointer press reaches egui");
    }

    #[test]
    fn two_fingers_run_the_camera_and_quiet_the_pointer() {
        let mut s = TouchState::default();
        let mut raw = frame(vec![touch(1, TouchPhase::Start, 10.0, None)]);
        press(&mut s, &mut raw);
        assert!(!s.gesture());
        let mut raw =
            frame(vec![touch(1, TouchPhase::Move, 20.0, None), Event::PointerMoved(pos2(20.0, 100.0)), touch(2, TouchPhase::Start, 80.0, None)]);
        press(&mut s, &mut raw);
        assert!(s.gesture(), "two fingers");
        let pointers: Vec<&Event> = raw.events.iter().filter(|e| matches!(e, Event::PointerMoved(_) | Event::PointerButton { .. })).collect();
        assert_eq!(pointers.len(), 1, "the emulated pointer is dropped but for one cancellation");
        assert!(matches!(pointers[0], Event::PointerButton { pressed: false, .. }), "the first finger's drag is ended once");
        let mut raw = frame(vec![touch(1, TouchPhase::Move, 25.0, None), Event::PointerMoved(pos2(25.0, 100.0))]);
        press(&mut s, &mut raw);
        assert!(raw.events.iter().all(|e| !matches!(e, Event::PointerMoved(_))), "no second cancellation");
    }

    #[test]
    fn the_gesture_ends_when_a_finger_lifts() {
        let mut s = TouchState::default();
        for e in [touch(1, TouchPhase::Start, 10.0, None), touch(2, TouchPhase::Start, 80.0, None)] {
            let mut raw = frame(vec![e]);
            press(&mut s, &mut raw);
        }
        assert!(s.gesture());
        let mut raw = frame(vec![
            touch(2, TouchPhase::End, 80.0, None),
            Event::PointerButton { pos: pos2(10.0, 100.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        ]);
        press(&mut s, &mut raw);
        assert!(!s.gesture());
        assert!(raw.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, .. })), "the first finger's own release reaches egui");
    }

    #[test]
    fn a_pencil_never_runs_the_camera_even_with_palms() {
        let mut s = TouchState::default();
        let mut raw = frame(vec![
            touch(1, TouchPhase::Start, 30.0, Some(0.4)),
            touch(2, TouchPhase::Start, 200.0, None),
            touch(3, TouchPhase::Start, 300.0, None),
        ]);
        press(&mut s, &mut raw);
        assert!(s.pencil_down());
        assert!(!s.gesture(), "palms next to the Pencil are not a gesture");
        // The Pencil is raised but the palms stay: they stay palms, so no gesture starts.
        let mut raw = frame(vec![touch(1, TouchPhase::End, 30.0, None)]);
        press(&mut s, &mut raw);
        assert!(!s.pencil_down());
        assert!(!s.gesture(), "the palms the Pencil leaves behind never become fingers");
    }

    #[test]
    fn a_finger_gesture_needs_two_actual_fingers() {
        let mut s = TouchState::default();
        let mut raw = frame(vec![touch(1, TouchPhase::Start, 30.0, Some(0.2)), touch(2, TouchPhase::Start, 200.0, None)]);
        press(&mut s, &mut raw);
        assert!(!s.gesture());
        let mut raw = frame(vec![touch(1, TouchPhase::End, 30.0, None)]);
        press(&mut s, &mut raw);
        assert!(!s.pencil_down());
        assert!(!s.gesture(), "one finger left");
    }

    #[test]
    fn a_cancel_clears_the_touch() {
        let mut s = TouchState::default();
        let mut raw = frame(vec![touch(1, TouchPhase::Start, 10.0, None)]);
        press(&mut s, &mut raw);
        let mut raw = frame(vec![touch(1, TouchPhase::Cancel, 10.0, None)]);
        press(&mut s, &mut raw);
        assert_eq!(s.touches.len(), 0);
    }
}
