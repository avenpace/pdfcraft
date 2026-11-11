use super::*;
use egui::{pos2, vec2};

const FRAME: f64 = 1.0 / 60.0;

fn press(p: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn touch(id: u64, phase: TouchPhase, p: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(id), phase, pos: p, force: None }
}

/// A frame's events through `f` at `now`, with a surface where every point is a marked place
/// (`zone`) or a text field (`text`), and egui's button as `down`.
fn run(f: &mut Fingers, now: f64, events: Vec<Event>, zone: bool, text: bool) -> Vec<Event> {
    let mut raw = egui::RawInput { events, ..Default::default() };
    let (z, t) = (move |_: Pos2| zone, move |_: Pos2| text);
    f.filter(&mut raw, now, &Surface { zone: &z, text_field: &t, egui_down: false });
    raw.events
}

fn presses(events: &[Event]) -> Vec<(Pos2, bool)> {
    events.iter().filter_map(|e| if let Event::PointerButton { pos, pressed, .. } = e { Some((*pos, *pressed)) } else { None }).collect()
}

fn scrolled(events: &[Event]) -> Vec2 {
    events.iter().fold(Vec2::ZERO, |s, e| if let Event::MouseWheel { delta, .. } = e { s + *delta } else { s })
}

/// A finger put down at `at` on frame 0, as a browser sends it: `pointerdown`'s press, then
/// `touchstart`'s press and touch.
fn finger_down(f: &mut Fingers, at: Pos2, zone: bool, text: bool) -> Vec<Event> {
    let mut out = run(f, 0.0, vec![press(at, true)], zone, text);
    out.extend(run(f, FRAME, vec![press(at, true), touch(1, TouchPhase::Start, at)], zone, text));
    out
}

/// The finger lifts at `at`, at `now`: `pointerup`'s release, then `touchend`'s.
fn finger_up(f: &mut Fingers, now: f64, at: Pos2, zone: bool) -> Vec<Event> {
    let mut out = run(f, now, vec![press(at, false)], zone, false);
    out.extend(run(f, now, vec![press(at, false), Event::PointerGone, touch(1, TouchPhase::End, at)], zone, false));
    out
}

fn fingers() -> Fingers {
    let mut f = Fingers::new(true);
    f.expect_touch();
    f
}

#[test]
fn a_mouse_passes_unchanged() {
    let mut f = Fingers::new(true);
    let a = pos2(100.0, 100.0);
    let events = vec![Event::PointerMoved(a), press(a, true), Event::PointerMoved(pos2(100.0, 300.0)), press(pos2(100.0, 300.0), false)];
    assert_eq!(run(&mut f, 0.0, events.clone(), true, true), events, "a mouse drag is a mouse drag, even over the pages");
    // Also after a finger touched: the mouse moving first makes presses the mouse's again.
    let mut f = fingers();
    assert_eq!(run(&mut f, 0.0, events.clone(), true, true), events);
}

#[test]
fn a_press_no_finger_confirms_is_handed_on_as_it_was() {
    let a = pos2(50.0, 60.0);
    // Pressed while touch was last, then released with no touch event: the mouse's click.
    let mut f = fingers();
    assert!(presses(&run(&mut f, 0.0, vec![press(a, true)], true, false)).is_empty(), "held back first");
    assert_eq!(presses(&run(&mut f, 0.1, vec![press(a, false)], true, false)), vec![(a, true), (a, false)]);
    // Moved with the button down and no touch: the mouse's drag, from where it was pressed.
    let mut f = fingers();
    run(&mut f, 0.0, vec![press(a, true)], true, false);
    let out = run(&mut f, 0.1, vec![Event::PointerMoved(pos2(50.0, 160.0))], true, false);
    assert_eq!(presses(&out), vec![(a, true)]);
    assert_eq!(scrolled(&out), Vec2::ZERO, "a mouse drag never scrolls");
    assert_eq!(presses(&run(&mut f, 0.2, vec![press(pos2(50.0, 160.0), false)], true, false)), vec![(pos2(50.0, 160.0), false)]);
}

#[test]
fn a_finger_dragged_scrolls_one_to_one_and_presses_nothing() {
    let mut f = fingers();
    let a = pos2(200.0, 400.0);
    let mut out = finger_down(&mut f, a, true, false);
    let mut at = a;
    for k in 1..=10 {
        at = a + vec2(-2.0 * k as f32, -30.0 * k as f32);
        out.extend(run(&mut f, FRAME * (1 + k) as f64, vec![Event::PointerMoved(at), touch(1, TouchPhase::Move, at)], true, false));
    }
    assert!(presses(&out).is_empty(), "no press reaches egui: {out:?}");
    assert!(
        out.iter().all(|e| !matches!(e, Event::PointerMoved(p) if *p != a)),
        "egui's pointer stays where the scroll began, inside the area it scrolls"
    );
    assert_eq!(scrolled(&out), at - a, "the content followed the finger");
    // Lifted at speed: the release is not a click either, and the glide goes on.
    let lifted = finger_up(&mut f, FRAME * 11.0, at, true);
    assert!(presses(&lifted).is_empty());
    let mut glide = Vec::new();
    for k in 1..=10 {
        glide.extend(run(&mut f, FRAME * (11 + k) as f64, Vec::new(), true, false));
    }
    let g = scrolled(&glide);
    assert!(g.y < -50.0 && g.x <= 0.0, "it glides on the way it went: {g:?}");
    assert!(glide.iter().all(|e| !matches!(e, Event::MouseWheel { source, .. } if *source != MouseWheelSource::Momentum)), "as momentum");
    assert!(glide.iter().any(|e| matches!(e, Event::PointerMoved(p) if *p == a)), "over the place the scroll began, so the same area takes it");
    // A new touch stops the glide at once.
    let stopped = run(&mut f, FRAME * 22.0, vec![press(a, true), touch(2, TouchPhase::Start, a)], true, false);
    assert!(stopped.iter().any(|e| matches!(e, Event::MouseWheel { phase: TouchPhase::End, source: MouseWheelSource::Momentum, .. })));
    assert!(
        run(&mut f, FRAME * 23.0, Vec::new(), true, false).iter().all(|e| !matches!(e, Event::MouseWheel { source: MouseWheelSource::Momentum, .. }))
    );
}

#[test]
fn a_finger_that_stops_before_lifting_does_not_glide() {
    let mut f = fingers();
    let a = pos2(200.0, 400.0);
    finger_down(&mut f, a, true, false);
    let b = a - vec2(0.0, 200.0);
    run(&mut f, 0.05, vec![Event::PointerMoved(b), touch(1, TouchPhase::Move, b)], true, false);
    finger_up(&mut f, 0.6, b, true);
    assert_eq!(scrolled(&run(&mut f, 0.62, Vec::new(), true, false)), Vec2::ZERO);
}

#[test]
fn a_tap_is_a_click_where_the_finger_went_down() {
    let mut f = fingers();
    let a = pos2(120.0, 80.0);
    let down = finger_down(&mut f, a, true, false);
    assert!(presses(&down).is_empty(), "not yet: it may become a scroll");
    let wobble = a + vec2(3.0, -4.0);
    let moved = run(&mut f, 0.05, vec![Event::PointerMoved(wobble), touch(1, TouchPhase::Move, wobble)], true, false);
    assert_eq!(scrolled(&moved), Vec2::ZERO, "within the slop");
    let up = finger_up(&mut f, 0.12, wobble, true);
    assert_eq!(presses(&up), vec![(a, true), (a, false)], "one click, in the frame it lifted: {up:?}");
    // The hover goes the next frame, once egui has had the click.
    assert!(!up.iter().any(|e| matches!(e, Event::PointerGone)));
    assert!(run(&mut f, 0.13, Vec::new(), true, false).iter().any(|e| matches!(e, Event::PointerGone)));
}

#[test]
fn a_long_press_then_a_drag_is_a_mouse_drag() {
    let mut f = fingers();
    let a = pos2(300.0, 300.0);
    finger_down(&mut f, a, true, false);
    assert!(presses(&run(&mut f, 0.3, Vec::new(), true, false)).is_empty(), "not long enough yet");
    let held = run(&mut f, LONG_PRESS + 0.01, Vec::new(), true, false);
    assert_eq!(presses(&held), vec![(a, true)], "the press arrives once held long enough");
    let b = a + vec2(150.0, 40.0);
    let dragged = run(&mut f, 0.6, vec![Event::PointerMoved(b), touch(1, TouchPhase::Move, b)], true, false);
    assert_eq!(scrolled(&dragged), Vec2::ZERO, "it drags, it doesn't scroll");
    assert!(dragged.contains(&Event::PointerMoved(b)));
    let up = finger_up(&mut f, 0.7, b, true);
    assert_eq!(presses(&up).first(), Some(&(b, false)), "and lets go where it is");
}

#[test]
fn two_fingers_pan_and_press_nothing() {
    let mut f = fingers();
    let (a, b) = (pos2(100.0, 300.0), pos2(300.0, 300.0));
    let mut out = finger_down(&mut f, a, true, false);
    out.extend(run(&mut f, 0.03, vec![touch(2, TouchPhase::Start, b)], true, false));
    // Apart (a pinch: the centre stays) and then together down by 50 (a pan).
    let (a2, b2) = (pos2(50.0, 300.0), pos2(350.0, 300.0));
    out.extend(run(&mut f, 0.05, vec![Event::PointerMoved(a2), touch(1, TouchPhase::Move, a2), touch(2, TouchPhase::Move, b2)], true, false));
    assert_eq!(scrolled(&out), Vec2::ZERO, "a pinch about its centre doesn't pan");
    let (a3, b3) = (a2 + vec2(0.0, 50.0), b2 + vec2(0.0, 50.0));
    out.extend(run(&mut f, 0.07, vec![Event::PointerMoved(a3), touch(1, TouchPhase::Move, a3), touch(2, TouchPhase::Move, b3)], true, false));
    assert_eq!(scrolled(&out), vec2(0.0, 50.0), "the centre moving pans");
    out.extend(run(&mut f, 0.8, vec![touch(2, TouchPhase::End, b3)], true, false));
    out.extend(finger_up(&mut f, 0.9, a3, true));
    assert!(presses(&out).is_empty(), "no press, no long press, no tap: {out:?}");
}

#[test]
fn a_text_field_scrolls_by_finger_and_other_widgets_get_the_press() {
    let a = pos2(40.0, 40.0);
    // Over a text field (found the frame after the press), a drag scrolls.
    let mut f = fingers();
    let mut out = finger_down(&mut f, a, false, true);
    let b = a - vec2(0.0, 100.0);
    out.extend(run(&mut f, 0.05, vec![Event::PointerMoved(b), touch(1, TouchPhase::Move, b)], false, true));
    assert!(presses(&out).is_empty());
    assert_eq!(scrolled(&out), b - a);
    // Over anything else (a slider, a splitter), the press goes on the next frame, unchanged.
    let mut f = fingers();
    let out = finger_down(&mut f, a, false, false);
    assert_eq!(presses(&out), vec![(a, true), (a, true)], "late by a frame, then the browser's second press");
    let out = run(&mut f, 0.05, vec![Event::PointerMoved(b), touch(1, TouchPhase::Move, b)], false, false);
    assert_eq!(scrolled(&out), Vec2::ZERO, "the widget has the drag");
}

#[test]
fn a_press_egui_already_has_stays_as_it_is() {
    // The very first touch on a device not known to be touch-first: `pointerdown` reached egui
    // as a mouse press before any touch event. That gesture stays a mouse's.
    let mut f = Fingers::new(true);
    let a = pos2(10.0, 10.0);
    assert_eq!(presses(&run(&mut f, 0.0, vec![press(a, true)], true, false)), vec![(a, true)]);
    let mut raw = egui::RawInput { events: vec![press(a, true), touch(1, TouchPhase::Start, a)], ..Default::default() };
    let yes = |_: Pos2| true;
    let no = |_: Pos2| false;
    f.filter(&mut raw, FRAME, &Surface { zone: &yes, text_field: &no, egui_down: true });
    assert_eq!(presses(&raw.events), vec![(a, true)]);
    // The next touch is a finger's.
    finger_up(&mut f, 0.1, a, true);
    run(&mut f, 0.2, vec![press(a, true), touch(3, TouchPhase::Start, a)], true, false);
    let b = a + vec2(0.0, 80.0);
    let out = run(&mut f, 0.25, vec![Event::PointerMoved(b), touch(3, TouchPhase::Move, b)], true, false);
    assert_eq!(scrolled(&out), b - a);
}

#[test]
fn switched_off_it_changes_nothing() {
    let mut f = Fingers::new(false);
    let a = pos2(1.0, 2.0);
    let events = vec![press(a, true), touch(1, TouchPhase::Start, a)];
    assert_eq!(run(&mut f, 0.0, events.clone(), true, true), events);
}
