//! Fingers on a touch screen (a phone, a tablet, a touch laptop): a drag scrolls, a tap clicks,
//! a long press then a drag selects, two fingers pinch-zoom and pan.
//!
//! egui hands a finger to the app twice: as `Event::Touch` and as emulated primary-button
//! presses, moves and releases. Left alone, a finger dragged over the pages is a mouse drag: it
//! selects text (or moves a comment) and the document never scrolls, and the same goes for a
//! text field, whose drag selects. So the window's input is read here before egui sees it
//! ([`Fingers::filter`]), and a finger press that lands where a finger should scroll is held
//! back until it is known what it is:
//!
//! - moved more than [`SLOP`] first: a scroll. The press never reaches egui; the finger's motion
//!   goes on as point-unit wheel input with touch phases (1:1, as a touchpad's, which egui and
//!   the page view apply unsmoothed), and after the lift it glides on and slows down
//!   ([`crate::fling::Fling`]) as a second, momentum-phase gesture, the way macOS sends one;
//! - lifted before that: a tap, delivered as a press and a release where the finger went down,
//!   in the frame it lifted (the web runs that frame inside the touch event, so a text field the
//!   tap focuses brings up the on-screen keyboard);
//! - held still for [`LONG_PRESS`]: the press is delivered then, and from there on the finger is
//!   a mouse with its button down (a drag selects text, moves a comment, reorders thumbnails);
//! - joined by a second finger: neither. egui's multi-touch gives the pinch's zoom (the page view
//!   anchors it at the pinch's centre), and the fingers' centre moving pans, as wheel input.
//!
//! Where a finger scrolls: the places the UI marks each pass with [`scroll_here`] (the page view,
//! the page thumbnails, the Organize grid), and every text field, single- or multi-line (egui
//! knows by the next frame which widget is under the press; until then the press waits).
//! Everywhere else a finger is left to egui: its scroll areas already scroll by a finger dragged
//! over buttons and labels, and sliders, splitters and grips act on the drag at once.
//!
//! Only a press that came from a finger is changed, never one from a mouse or a touchpad. The
//! browser sends its `pointerdown` (which reaches egui as a press) before the `touchstart` that
//! says it is a finger, so a press counts as a finger's while the last input was touch (once a
//! finger has touched, until the mouse moves; [`Fingers::expect_touch`] starts it on for a
//! touch-first device), and a held press that turns out to have no finger behind it (the mouse
//! moved or let go first) is handed on as the mouse press it was. Not on Windows' desktop app:
//! winit reports a pen there as touch, and a pen draws and selects.

use crate::fling::Fling;
use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase, Vec2};

/// How far (points) a finger may move and still tap or long-press.
pub(crate) const SLOP: f32 = 8.0;
/// How long (seconds) a finger held still takes to become a press that selects.
pub(crate) const LONG_PRESS: f64 = 0.5;
/// Fingers tracked at once; more are ignored (they are still passed on to egui).
const MAX_FINGERS: usize = 10;
/// Places marked per pass; more are ignored.
const MAX_ZONES: usize = 512;

/// A finger's press held back, where and when it went down.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    pos: Pos2,
    at: f64,
    modifiers: Modifiers,
}

impl Press {
    fn event(self, pressed: bool) -> Event {
        Event::PointerButton { pos: self.pos, button: PointerButton::Primary, pressed, modifiers: self.modifiers }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    /// No finger gesture: input passes as it is.
    Idle,
    /// A finger pressed outside the marked places, this frame: egui looks under it, and the next
    /// frame says whether it is a text field's (a scroll gesture) or something else's (handed on).
    Deciding(Press),
    /// A finger down and not yet moved past [`SLOP`]; `touched` once a touch event confirmed it.
    Pending { press: Press, last: Pos2, touched: bool },
    /// A finger scrolling: where it was last, and where it went down. egui's pointer stays
    /// there, so the wheel input keeps going to the area the finger started on even once the
    /// finger has left it (over a toolbar, past the window's edge).
    Pan { last: Pos2, origin: Pos2 },
    /// Input passes as it is until the fingers lift (a long press delivered, a press egui
    /// already had) or, for a press no finger confirmed, until the button is up.
    Held { finger: bool },
    /// Two or more fingers: the fingers' centre last frame, and how many there were.
    Pinch(Option<(usize, Pos2)>),
}

/// The momentum after a scroll: where the scroll's pointer is (the wheel goes to what is under it).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    at: Pos2,
    started: bool,
}

/// What the window looked like last pass, for a press arriving now.
pub(crate) struct Surface<'a> {
    /// Whether a finger scrolls at this point: a place marked with [`scroll_here`].
    pub zone: &'a dyn Fn(Pos2) -> bool,
    /// Whether egui found a text field under the press held back last frame, at this point.
    pub text_field: &'a dyn Fn(Pos2) -> bool,
    /// egui has the primary button down already (a press handed on before a finger said so).
    pub egui_down: bool,
}

/// The touch gesture in progress, kept from frame to frame. See the module docs.
pub(crate) struct Fingers {
    /// Whether finger presses are read at all (see the module docs for Windows).
    pub(crate) enabled: bool,
    /// The last input was touch, so a press now is a finger's.
    touch: bool,
    /// The fingers down, by device and id, and where each is.
    down: Vec<(TouchDeviceId, TouchId, Pos2)>,
    gesture: Gesture,
    /// Measures the scroll and runs its glide.
    fling: Fling,
    glide: Option<Glide>,
    /// A tap's release went to egui in this frame: the hover it left goes next frame.
    gone_next: bool,
}

impl Default for Fingers {
    fn default() -> Self {
        Self::new(cfg!(target_arch = "wasm32") || !cfg!(windows))
    }
}

impl Fingers {
    pub(crate) fn new(enabled: bool) -> Self {
        Self { enabled, touch: false, down: Vec::new(), gesture: Gesture::Idle, fling: Fling::new(true), glide: None, gone_next: false }
    }

    /// A device whose main pointer is a finger: the first press already counts as a finger's,
    /// before any touch event has said so.
    pub(crate) fn expect_touch(&mut self) {
        self.touch = true;
    }

    /// Reads this frame's `raw` input at `now` (seconds) and rewrites it as described in the
    /// module docs. How long until the next frame is wanted (a long press coming, a glide), if
    /// one is.
    pub(crate) fn filter(&mut self, raw: &mut egui::RawInput, now: f64, surface: &Surface<'_>) -> Option<f64> {
        if !self.enabled || !now.is_finite() {
            return None;
        }
        let events = std::mem::take(&mut raw.events);
        let touched = events.iter().any(|e| matches!(e, Event::Touch { .. }));
        if touched {
            self.touch = true;
        } else if self.gesture == Gesture::Idle && self.down.is_empty() && events.iter().any(|e| matches!(e, Event::PointerMoved(_))) {
            // The mouse moved: presses are the mouse's again.
            self.touch = false;
        }
        let mut out = Vec::with_capacity(events.len() + 6);
        if std::mem::take(&mut self.gone_next) {
            out.push(Event::PointerGone);
        }
        // Last frame's press outside the marked places: egui has looked under it now.
        if let Gesture::Deciding(press) = self.gesture {
            if (surface.text_field)(press.pos) {
                self.gesture = Gesture::Pending { press, last: press.pos, touched: touched || !self.down.is_empty() };
            } else {
                out.push(press.event(true));
                self.gesture = Gesture::Held { finger: true };
            }
        }
        if self.glide.is_some() && events.iter().any(stops_glide) {
            self.end_glide(&mut out);
        }
        for event in events {
            self.event(event, now, surface, touched, &mut out);
        }
        self.after_events(now, &mut out);
        raw.events = out;
        match self.gesture {
            _ if self.glide.is_some() || self.gone_next => Some(0.0),
            Gesture::Pending { press, .. } => Some((LONG_PRESS - (now - press.at)).max(0.0)),
            Gesture::Deciding(_) => Some(0.0),
            _ => None,
        }
    }

    fn event(&mut self, event: Event, now: f64, surface: &Surface<'_>, touched: bool, out: &mut Vec<Event>) {
        match event {
            Event::Touch { device_id, id, phase, pos, .. } => {
                out.push(event);
                self.touch_event(device_id, id, phase, pos, now, out);
            }
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } => match self.gesture {
                Gesture::Idle => {
                    let finger = touched || self.touch;
                    if !finger || surface.egui_down || !pos.is_finite() {
                        if finger {
                            // egui has the press already (it came before any touch said it was
                            // a finger): this gesture stays a mouse's.
                            self.gesture = Gesture::Held { finger: true };
                        }
                        out.push(event);
                        return;
                    }
                    let press = Press { pos, at: now, modifiers };
                    // So that egui looks at what is under it.
                    out.push(Event::PointerMoved(pos));
                    self.gesture = if (surface.zone)(pos) {
                        Gesture::Pending { press, last: pos, touched: touched || !self.down.is_empty() }
                    } else {
                        Gesture::Deciding(press)
                    };
                }
                Gesture::Held { .. } => out.push(event),
                // The same finger's second press (`pointerdown`, then `touchstart`), or another.
                _ => {}
            },
            Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } => match self.gesture {
                Gesture::Deciding(press) | Gesture::Pending { press, touched: false, .. } if self.down.is_empty() => {
                    // No finger behind it: the mouse's click, as it was.
                    self.touch = false;
                    out.push(press.event(true));
                    out.push(event);
                    self.gesture = Gesture::Idle;
                }
                Gesture::Held { finger } => {
                    out.push(event);
                    if !finger || (self.down.is_empty() && !touched) {
                        self.gesture = Gesture::Idle;
                    }
                }
                Gesture::Idle => out.push(event),
                // A finger's release: the touch event that ends it decides what it was.
                _ => {}
            },
            Event::PointerMoved(p) => {
                match self.gesture {
                    Gesture::Deciding(press) | Gesture::Pending { press, touched: false, .. } if self.down.is_empty() && !touched => {
                        // The mouse moved with its button down: it drags, as it did.
                        self.touch = false;
                        out.push(press.event(true));
                        self.gesture = Gesture::Held { finger: false };
                    }
                    Gesture::Deciding(press) | Gesture::Pending { press, .. } => {
                        if p.is_finite() && (p - press.pos).length() > SLOP {
                            self.scroll(p - press.pos, TouchPhase::Start, now, out);
                            self.gesture = Gesture::Pan { last: p, origin: press.pos };
                            out.push(Event::PointerMoved(press.pos));
                            return;
                        } else if let Gesture::Pending { last, .. } = &mut self.gesture {
                            *last = p;
                        }
                    }
                    Gesture::Pan { last, origin } => {
                        if p.is_finite() {
                            self.scroll(p - last, TouchPhase::Move, now, out);
                            self.gesture = Gesture::Pan { last: p, origin };
                        }
                        out.push(Event::PointerMoved(origin));
                        return;
                    }
                    _ => {}
                }
                out.push(event);
            }
            // The hover a lifted finger leaves goes once its gesture is over (a glide still needs it).
            Event::PointerGone if self.glide.is_some() || !matches!(self.gesture, Gesture::Idle | Gesture::Held { .. }) => {}
            _ => out.push(event),
        }
    }

    fn touch_event(&mut self, device: TouchDeviceId, id: TouchId, phase: TouchPhase, pos: Pos2, now: f64, out: &mut Vec<Event>) {
        let known = self.down.iter().position(|(d, i, _)| *d == device && *i == id);
        match phase {
            TouchPhase::Start | TouchPhase::Move => match known {
                Some(k) => {
                    if let Some(f) = self.down.get_mut(k)
                        && pos.is_finite()
                    {
                        f.2 = pos;
                    }
                }
                None if phase == TouchPhase::Start && self.down.len() < MAX_FINGERS && pos.is_finite() => self.down.push((device, id, pos)),
                None => {}
            },
            TouchPhase::End | TouchPhase::Cancel => {
                if let Some(k) = known {
                    self.down.remove(k);
                }
            }
        }
        if let Gesture::Pending { touched, .. } = &mut self.gesture {
            *touched = true;
        }
        if self.down.len() >= 2 {
            match self.gesture {
                Gesture::Pan { .. } => {
                    // The scroll stops where it is; the fingers pan from here.
                    self.scroll(Vec2::ZERO, TouchPhase::Cancel, now, out);
                    self.pinch_start(now, out);
                }
                Gesture::Deciding(_) | Gesture::Pending { .. } => self.pinch_start(now, out),
                _ => {}
            }
        }
        if matches!(phase, TouchPhase::End | TouchPhase::Cancel) && self.down.is_empty() {
            self.lift(now, out);
        }
    }

    fn pinch_start(&mut self, now: f64, out: &mut Vec<Event>) {
        self.scroll(Vec2::ZERO, TouchPhase::Start, now, out);
        self.gesture = Gesture::Pinch(None);
    }

    /// The last finger lifted.
    fn lift(&mut self, now: f64, out: &mut Vec<Event>) {
        match self.gesture {
            Gesture::Deciding(press) | Gesture::Pending { press, .. } => {
                // A tap: a click where the finger went down.
                out.push(Event::PointerMoved(press.pos));
                out.push(press.event(true));
                out.push(press.event(false));
                self.gone_next = true;
            }
            Gesture::Pan { origin, .. } => {
                self.scroll(Vec2::ZERO, TouchPhase::End, now, out);
                if self.fling.gliding() {
                    self.glide = Some(Glide { at: origin, started: false });
                } else {
                    out.push(Event::PointerGone);
                }
            }
            Gesture::Pinch(_) => {
                self.scroll(Vec2::ZERO, TouchPhase::End, now, out);
                out.push(Event::PointerGone);
            }
            Gesture::Idle | Gesture::Held { .. } => {}
        }
        self.gesture = Gesture::Idle;
    }

    /// The finger's scroll as wheel input, measured for the glide.
    fn scroll(&mut self, delta: Vec2, phase: TouchPhase, now: f64, out: &mut Vec<Event>) {
        let event = wheel(delta, phase, MouseWheelSource::Trackpad);
        self.fling.read(std::slice::from_ref(&event), now, true);
        out.push(event);
    }

    fn after_events(&mut self, now: f64, out: &mut Vec<Event>) {
        match self.gesture {
            // Held still long enough: the press, where it went down.
            Gesture::Pending { press, last, touched } if now - press.at >= LONG_PRESS => {
                out.push(Event::PointerMoved(press.pos));
                out.push(press.event(true));
                if last != press.pos {
                    out.push(Event::PointerMoved(last));
                }
                self.gesture = Gesture::Held { finger: touched || !self.down.is_empty() };
            }
            // Two fingers: their centre's motion pans; a finger added or lifted moves the centre
            // without panning.
            Gesture::Pinch(last) if self.down.len() >= 2 => {
                let n = self.down.len();
                let sum = self.down.iter().fold(Vec2::ZERO, |s, (_, _, p)| s + p.to_vec2());
                let centre = (sum / n as f32).to_pos2();
                if let Some((m, before)) = last
                    && m == n
                    && centre != before
                {
                    self.scroll(centre - before, TouchPhase::Move, now, out);
                }
                self.gesture = Gesture::Pinch(Some((n, centre)));
            }
            Gesture::Pinch(_) => self.gesture = Gesture::Pinch(None),
            _ => {}
        }
        if let Some(glide) = self.glide.as_mut() {
            let moved = self.fling.step(now);
            if self.fling.gliding() {
                let phase = if glide.started { TouchPhase::Move } else { TouchPhase::Start };
                glide.started = true;
                out.push(Event::PointerMoved(glide.at));
                out.push(wheel(moved, phase, MouseWheelSource::Momentum));
            } else {
                self.end_glide(out);
            }
        }
    }

    fn end_glide(&mut self, out: &mut Vec<Event>) {
        if let Some(glide) = self.glide.take() {
            self.fling.stop();
            if glide.started {
                out.push(wheel(Vec2::ZERO, TouchPhase::End, MouseWheelSource::Momentum));
            }
            out.push(Event::PointerGone);
        }
    }
}

/// Input that stops a glide: a press, a finger, a key, the user's own wheel.
fn stops_glide(event: &Event) -> bool {
    matches!(
        event,
        Event::PointerButton { pressed: true, .. }
            | Event::Touch { phase: TouchPhase::Start, .. }
            | Event::Key { pressed: true, .. }
            | Event::MouseWheel { .. }
    )
}

fn wheel(delta: Vec2, phase: TouchPhase, source: MouseWheelSource) -> Event {
    Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers: Modifiers::NONE, source }
}

/// The places marked this pass, and the pass.
#[derive(Clone, Default)]
struct Zones(u64, Vec<(egui::LayerId, Rect)>);

fn zones_id() -> egui::Id {
    egui::Id::unique("pdfcraft-finger-zones")
}

/// Marks `rect` on `layer`, as drawn this pass, as a place where a finger drag scrolls (and a
/// long press then a drag does what a mouse drag does). See the module docs.
pub(crate) fn scroll_here(ctx: &egui::Context, layer: egui::LayerId, rect: Rect) {
    if !rect.is_positive() || !rect.is_finite() {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let zones = d.get_temp_mut_or_default::<Zones>(zones_id());
        if zones.0 != pass {
            *zones = Zones(pass, Vec::new());
        }
        if zones.1.len() < MAX_ZONES {
            zones.1.push((layer, rect));
        }
    });
}

/// Whether a finger pressed at `pos` scrolls there: a place the last pass marked, on the layer on
/// top at `pos`.
fn in_zone(ctx: &egui::Context, pos: Pos2) -> bool {
    let Zones(pass, zones) = ctx.data(|d| d.get_temp::<Zones>(zones_id())).unwrap_or_default();
    if pass.saturating_add(1) < ctx.cumulative_pass_nr() {
        return false;
    }
    let top = ctx.layer_id_at(pos);
    zones.iter().any(|(layer, r)| r.contains(pos) && top == Some(*layer))
}

/// Whether egui found a text field under the pointer last pass, at `pos`, on the layer on top.
fn on_text_field(ctx: &egui::Context, pos: Pos2) -> bool {
    let ids: Vec<egui::Id> = ctx.interaction_snapshot(|s| s.contains_pointer.iter().copied().take(64).collect());
    let top = ctx.layer_id_at(pos);
    ids.into_iter().any(|id| {
        egui::text_edit::TextEditState::load(ctx, id).is_some()
            && ctx.read_response(id).is_some_and(|r| r.interact_rect.contains(pos) && top == Some(r.layer_id))
    })
}

impl crate::PdfCraftApp {
    /// Reads fingers in this frame's `raw` input before egui does ([`Fingers::filter`]).
    pub(crate) fn filter_fingers(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let now = raw.time.unwrap_or_else(|| ctx.input(|i| i.time + f64::from(i.predicted_dt)));
        let zone = |p: Pos2| in_zone(ctx, p);
        let text_field = |p: Pos2| on_text_field(ctx, p);
        let surface = Surface { zone: &zone, text_field: &text_field, egui_down: ctx.input(|i| i.pointer.primary_down()) };
        if let Some(after) = self.fingers.filter(raw, now, &surface) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(after.clamp(0.0, 1.0)));
        }
    }

    /// The device's main pointer is a finger (a phone or tablet browser): see
    /// [`Fingers::expect_touch`].
    pub fn expect_touch(&mut self) {
        self.fingers.expect_touch();
    }
}

#[cfg(test)]
mod tests;
