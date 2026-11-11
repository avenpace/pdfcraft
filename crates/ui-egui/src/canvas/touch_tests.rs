//! Fingers on the page view, end to end: the window's input goes through the app's raw-input
//! step (`PdfCraftApp::prepare_raw_input`, which eframe's `raw_input_hook` runs and kittest
//! doesn't), then through egui and the pages.

use super::tests::text_pages_pdf;
use crate::PdfCraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase, Vec2, pos2, vec2};

type H = egui_kittest::Harness<'static, PdfCraftApp>;

const FRAME: f64 = 1.0 / 60.0;

/// The app on a three-page document whose first page's text is read, its first page on screen,
/// and the time to start at.
fn harness() -> (H, Rect, f64) {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    let mut on_screen = None;
    for _ in 0..800 {
        h.run_steps(2);
        on_screen = h.state().views[0].page_screen_rect(0).filter(|_| h.state().views[0].texts.contains_key(&0));
        if on_screen.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = on_screen.expect("the first page and its text are there");
    let t = h.ctx.input(|i| i.time) + 1.0;
    (h, r, t)
}

/// One frame of window input at `t`, as eframe hands it to the app.
fn frame(h: &mut H, t: f64, events: Vec<Event>) {
    let ctx = h.ctx.clone();
    let mut raw = egui::RawInput { time: Some(t), events, ..Default::default() };
    h.state_mut().prepare_raw_input(&ctx, &mut raw);
    let input = h.input_mut();
    input.time = Some(t);
    input.events.extend(raw.events);
    h.step();
}

fn press(p: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn touch(id: u64, phase: TouchPhase, p: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(7), id: TouchId(id), phase, pos: p, force: None }
}

/// A finger down at `p`, as a phone's browser sends it: `pointerdown`, then `touchstart`. The
/// web app tells a touch-first device so (`expect_touch`), so even the first `pointerdown` counts
/// as a finger's.
fn finger_down(h: &mut H, t: f64, p: Pos2) {
    h.state_mut().expect_touch();
    frame(h, t, vec![press(p, true)]);
    frame(h, t + 0.001, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
}

fn finger_move(h: &mut H, t: f64, p: Pos2) {
    frame(h, t, vec![Event::PointerMoved(p), touch(1, TouchPhase::Move, p)]);
}

/// The finger lifts at `p`: `pointerup`, then `touchend`.
fn finger_up(h: &mut H, t: f64, p: Pos2) {
    frame(h, t, vec![press(p, false)]);
    frame(h, t + 0.001, vec![press(p, false), Event::PointerGone, touch(1, TouchPhase::End, p)]);
}

/// Where the first line of text ("Alpha one", 18 pt at 72, 700 on a 612 × 792 page) is, at `x`
/// points along its line.
fn on_line(r: Rect, x: f32) -> Pos2 {
    pos2(r.left() + (72.0 + x) / 612.0 * r.width(), r.top() + (792.0 - 706.0) / 792.0 * r.height())
}

fn scroll(h: &H) -> Vec2 {
    h.state().views[0].last_scroll
}

#[test]
fn a_finger_dragged_over_the_text_scrolls_and_selects_nothing() {
    let (mut h, r, mut t) = harness();
    h.state_mut().views[0].select_all();
    let before = scroll(&h);
    // From the line of text up and to the left, as a reader flicks to read on.
    let start = on_line(r, 20.0);
    finger_down(&mut h, t, start);
    let mut at = start;
    for k in 1..=10 {
        t += FRAME;
        at = start + vec2(0.0, -30.0 * k as f32);
        finger_move(&mut h, t, at);
    }
    let dragged = scroll(&h) - before;
    assert!((dragged.y - 300.0).abs() < 1.0, "the page follows the finger 1:1: {dragged:?}");
    let v = &h.state().views[0];
    assert!(v.all_text && v.selection.is_none(), "nothing selected, and no press reached the page (Select All is kept)");
    // Lifted at speed: the page glides on, and slows down.
    t += FRAME;
    finger_up(&mut h, t, at);
    let lifted = scroll(&h);
    let mut steps = Vec::new();
    for _ in 0..20 {
        t += FRAME;
        frame(&mut h, t, Vec::new());
        steps.push(scroll(&h).y);
    }
    let glide = scroll(&h).y - lifted.y;
    assert!(glide > 50.0, "it keeps going after the lift: {glide}");
    let (first, last) = (steps[1] - steps[0], steps[19] - steps[18]);
    assert!(last < first, "and decelerates: {first} then {last}");
    assert!(h.state().views[0].all_text, "the lift was no click either");
}

#[test]
fn a_tap_on_the_page_is_still_a_click() {
    let (mut h, r, t) = harness();
    h.state_mut().views[0].select_all();
    let before = scroll(&h);
    let at = on_line(r, 20.0);
    finger_down(&mut h, t, at);
    finger_move(&mut h, t + 0.04, at + vec2(2.0, 3.0));
    finger_up(&mut h, t + 0.1, at + vec2(2.0, 3.0));
    frame(&mut h, t + 0.12, Vec::new());
    assert!(!h.state().views[0].all_text, "the tap pressed the page, as a click does (it ends Select All)");
    assert_eq!(scroll(&h), before, "and scrolled nothing");
}

#[test]
fn a_long_press_then_a_drag_selects_text() {
    let (mut h, r, mut t) = harness();
    let before = scroll(&h);
    let start = on_line(r, -6.0);
    finger_down(&mut h, t, start);
    // Held still past the long press, with frames in between as the app asks for them.
    for _ in 0..6 {
        t += 0.1;
        frame(&mut h, t, Vec::new());
    }
    let end = on_line(r, 50.0);
    for k in 1..=6 {
        t += FRAME;
        finger_move(&mut h, t, start + (end - start) * (k as f32 / 6.0));
    }
    t += FRAME;
    finger_up(&mut h, t, end);
    frame(&mut h, t + FRAME, Vec::new());
    let text = h.state().views[0].selected_text().unwrap_or_default();
    assert!(text.contains("Alpha"), "the drag after the long press selected: {text:?}");
    assert_eq!(scroll(&h), before, "and didn't scroll");
}

#[test]
fn two_fingers_pinch_zoom_the_page() {
    let (mut h, r, mut t) = harness();
    let zoom = h.state().views[0].zoom;
    let c = r.center();
    let (mut a, mut b) = (c - vec2(60.0, 0.0), c + vec2(60.0, 0.0));
    finger_down(&mut h, t, a);
    t += FRAME;
    frame(&mut h, t, vec![touch(2, TouchPhase::Start, b)]);
    for _ in 0..6 {
        t += FRAME;
        a -= vec2(15.0, 0.0);
        b += vec2(15.0, 0.0);
        frame(&mut h, t, vec![Event::PointerMoved(a), touch(1, TouchPhase::Move, a), touch(2, TouchPhase::Move, b)]);
    }
    let zoomed = h.state().views[0].zoom;
    assert!(zoomed > zoom * 1.5, "spread apart, the fingers zoomed in: {zoom} to {zoomed}");
    t += FRAME;
    frame(&mut h, t, vec![touch(2, TouchPhase::End, b)]);
    finger_up(&mut h, t + FRAME, a);
    let v = &h.state().views[0];
    assert!(v.selection.is_none(), "and selected nothing");
}

#[test]
fn a_mouse_drag_still_selects_text() {
    let (mut h, r, mut t) = harness();
    let before = scroll(&h);
    let (start, end) = (on_line(r, -6.0), on_line(r, 50.0));
    frame(&mut h, t, vec![Event::PointerMoved(start)]);
    t += FRAME;
    frame(&mut h, t, vec![press(start, true)]);
    for k in 1..=6 {
        t += FRAME;
        frame(&mut h, t, vec![Event::PointerMoved(start + (end - start) * (k as f32 / 6.0))]);
    }
    t += FRAME;
    frame(&mut h, t, vec![press(end, false)]);
    frame(&mut h, t + FRAME, Vec::new());
    let text = h.state().views[0].selected_text().unwrap_or_default();
    assert!(text.contains("Alpha"), "a mouse drag selects at once, as before: {text:?}");
    assert_eq!(scroll(&h), before, "and doesn't scroll");
}
