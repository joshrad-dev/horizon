//! Touch screens (iOS, iPadOS): gesture controls in the manner of the classic Pocket Edition,
//! with no buttons and no crosshair. On iPhone Duo held upright the deck below the fold has the
//! two sides as pads around its page view (game/deck.rs), and the game's whole view looks.
//!
//! In play the game's view has two halves:
//! - **Left**: a floating stick wherever the thumb lands (the virtual pad's left stick); pushed
//!   to its edge it sprints.
//! - **Right**: a swipe up jumps and a swipe down crouches, both until the finger lifts; a swipe
//!   across starts turning the view (then it turns every way); a tap opens the inventory;
//!   holding still mines (or attacks) whatever is under the finger; a double tap uses or places
//!   at the finger.
//! - **Both**: a double tap with a finger on each half pauses.
//!
//! The hotbar takes taps to select a slot. Actions press the virtual pad's buttons, so they act
//! as a controller's would, with its bindings.
//!
//! Everywhere else (the title, menus, the inventory) the first finger is the mouse: down is a
//! left press where it touches, moving drags, up releases; two fingers scroll. A tap outside a
//! menu opened in play closes it.
//!
//! Distances and speeds are in points (window pixels over the scale factor), so the gestures
//! feel the same on every screen whatever its GUI scale.
//!
//! The launcher's other scheme, **Buttons**, puts on-screen buttons and a joystick over play
//! instead (`horizon_client::touch`, laid out in the launcher's editor and saved in
//! [`LAYOUT_FILE`]): the fingers that land in play go to them, and they press the virtual pad's
//! buttons, turn the view, and send the keys of the player's own buttons.
use super::event_loop::EventLoopCtl;
use super::{Game, Screen};
use horizon_client::gamepad::Button;
use horizon_client::hud::HudBatch;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use winit::event::{
    DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
};
use winit::window::WindowId;

/// A press held this long without moving mines (holds the attack trigger).
const HOLD: Duration = Duration::from_millis(320);
/// A tap lifts sooner than this.
const TAP: Duration = Duration::from_millis(280);
/// Two taps this close together are a double tap.
const DOUBLE_TAP: Duration = Duration::from_millis(350);
/// Taps on the two halves this close together are one tap with both hands.
const BOTH_SIDES: Duration = Duration::from_millis(180);
/// A finger moving less than this (points) from where it landed is still a tap.
const TAP_SLOP: f64 = 10.0;
/// A swipe up or down: this far (points), more vertical than horizontal.
const SWIPE_DISTANCE: f64 = 28.0;
/// The stick's reach from where the thumb landed, in points.
const STICK_REACH: f64 = 45.0;
/// Pushed this far the stick sprints.
const SPRINT_AT: f64 = 0.92;
/// The view's turn per point of finger travel, in mouse counts.
const LOOK_SPEED: f64 = 1.6;
/// The aim stays at the finger this long after a tap, for the tick that uses it.
const AIM_LINGER: Duration = Duration::from_millis(250);
/// Without the deck, the thumb walks from this corner of the view (fractions of its width and
/// height, from the lower left); the rest of the view looks and acts.
const WALK_WIDTH: f32 = 0.33;
const WALK_HEIGHT: f32 = 0.6;
/// The Buttons scheme's layout, in the game folder.
pub(super) const LAYOUT_FILE: &str = "horizon-touch-layout.json";
/// Degrees of turn per mouse count at the default sensitivity (`MouseHandler.turnPlayer`).
const DEGREES_PER_COUNT: f32 = 0.15;

pub(super) fn layout_path() -> std::path::PathBuf {
    Game::game_dir().join(LAYOUT_FILE)
}

/// Which half of the game's view a finger landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    /// Walking: the stick.
    Walk,
    /// Looking and acting.
    Act,
}

/// What one finger is doing.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// The left stick, centred where the thumb landed.
    Stick,
    /// On the acting half, not yet a tap, a swipe, a look or a hold.
    Pending,
    /// Turning the view.
    Look,
    /// Swiped up: jumping (swimming up, flying up) until it lifts.
    Jump,
    /// Swiped down: crouching until it lifts.
    Crouch,
    /// Held still: mining at the finger.
    Mine,
    /// The hotbar: nothing more once the slot is chosen.
    Hotbar,
    /// The mouse, in menus.
    Mouse,
    /// A second finger in a menu: scrolls with the first.
    Scroll,
    /// Done (on the deck, play ended under it): ignored until it lifts.
    Ignored,
    /// The Buttons scheme's: the on-screen controls have it.
    Buttons,
}

#[derive(Clone, Debug)]
struct Finger {
    side: Side,
    /// On the deck's pads (iPhone Duo upright) rather than the game's view: it acts at the
    /// view's centre.
    on_deck: bool,
    kind: Kind,
    /// Where it landed and where it is: points, and GUI pixels for aiming and drawing.
    start: [f64; 2],
    at: [f64; 2],
    at_gui: [f32; 2],
    since: Instant,
}

impl Finger {
    fn travelled(&self) -> f64 {
        distance(self.start, self.at)
    }
}

#[derive(Default)]
pub(super) struct TouchControls {
    fingers: HashMap<u64, Finger>,
    /// Pad buttons tapped since the last frame: the pads read them down once, and they are let
    /// go the frame after (`releasing`).
    taps: Vec<Button>,
    releasing: Vec<Button>,
    /// The touch screen has been used.
    seen: bool,
    /// The last frame was in play.
    was_playing: bool,
    /// The focused text field's box, in window pixels (x, y, width, height).
    text_area: Option<[i32; 4]>,
    /// The on-screen keyboard is wanted: the text field was tapped.
    keyboard: bool,
    /// The last tap on each half (where, in points), for double taps.
    last_tap: [Option<(Instant, [f64; 2])>; 2],
    /// The last tap with both hands, for the pause's double tap.
    last_both: Option<Instant>,
    /// A single tap's inventory, opened at this time if no second tap comes first.
    inventory_at: Option<Instant>,
    /// Where the pick aims instead of the view's centre: a point of the game's GUI, until when.
    aim: Option<([f32; 2], Option<Instant>)>,
    /// The field of view of the last frame, for aiming at the finger.
    pub(super) fov: f32,
    /// The Buttons scheme's controls, when the launcher chose it.
    buttons: Option<Box<horizon_client::touch::TouchControls>>,
    /// The launcher's choice has been read.
    scheme_read: bool,
}

/// A touch's or the pointer's place, from the window's corner to the safe area's, where the
/// menus and HUD are laid out (`hud::set_safe_area`). Every other event stays as it is.
pub(super) fn into_safe_area(event: WindowEvent, surface: Option<[u32; 2]>) -> WindowEvent {
    let Some(surface) = surface else {
        return event;
    };
    let [left, top, ..] = horizon_client::hud::safe_insets(surface);
    if left == 0 && top == 0 {
        return event;
    }
    let (dx, dy) = (f64::from(left), f64::from(top));
    match event {
        WindowEvent::CursorMoved {
            device_id,
            position,
        } => WindowEvent::CursorMoved {
            device_id,
            position: winit::dpi::PhysicalPosition::new(position.x - dx, position.y - dy),
        },
        WindowEvent::Touch(mut touch) => {
            touch.location.x -= dx;
            touch.location.y -= dy;
            WindowEvent::Touch(touch)
        }
        event => event,
    }
}

/// A touch as the mouse: where it is, then a left press or release.
pub(super) fn as_mouse(touch: Touch) -> Vec<WindowEvent> {
    let device_id = DeviceId::dummy();
    let moved = WindowEvent::CursorMoved {
        device_id,
        position: touch.location,
    };
    let button = |state| WindowEvent::MouseInput {
        device_id,
        state,
        button: MouseButton::Left,
    };
    match touch.phase {
        TouchPhase::Started => vec![moved, button(ElementState::Pressed)],
        TouchPhase::Moved => vec![moved],
        TouchPhase::Ended | TouchPhase::Cancelled => vec![moved, button(ElementState::Released)],
    }
}

/// A touch the pointer of a connected mouse made (a click, a trackpad press): the mouse's own
/// input (game/hardware_input.rs) has its buttons and place already.
pub(super) fn is_pointer(touch: &Touch) -> bool {
    let ended = matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled);
    horizon_ios_input::pointer_touch(touch.id, ended) && horizon_ios_input::mouse_connected()
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Game<'_> {
    /// The touch controls are on: iOS, or a touch screen seen elsewhere.
    pub(super) fn touch_controls(&self) -> bool {
        cfg!(target_os = "ios") || self.touch.seen
    }

    /// The touch controls take the crosshair away: the gestures aim at the finger, as do the
    /// buttons' tap schemes.
    pub(super) fn touch_hides_crosshair(&self) -> bool {
        self.touch_controls()
            && !self.hardware.active()
            && !self.touch.buttons.as_ref().is_some_and(|b| b.crosshair())
    }

    /// The Buttons scheme's controls, read with the launcher's choice the first time.
    fn touch_buttons_load(&mut self) {
        // Only on a touch screen: a desktop never shows them until a finger touches it.
        if self.touch.scheme_read || !self.touch_controls() {
            return;
        }
        self.touch.scheme_read = true;
        if super::launcher::buttons_chosen() {
            let settings = horizon_client::touch::Settings::load(&layout_path());
            self.touch.buttons = Some(Box::new(horizon_client::touch::TouchControls::new(
                settings,
            )));
        }
    }

    /// The launcher may have changed the scheme or the layout: read them again before the
    /// next frame in play.
    pub(super) fn touch_reload_scheme(&mut self) {
        if let Some(buttons) = &mut self.touch.buttons {
            buttons.clear();
        }
        self.touch.buttons = None;
        self.touch.scheme_read = false;
    }

    /// A key the touch buttons press or let go: a key's name (`key.keyboard.q`) or a key
    /// mapping's (`action:key.chat`, whatever key it is bound to), as the keyboard sends it.
    fn touch_key(&mut self, event_loop: &dyn EventLoopCtl, name: &str, down: bool) {
        let name = match name.strip_prefix("action:") {
            Some(mapping) => self.settings.keys.get(mapping).to_owned(),
            None => name.to_owned(),
        };
        let Some(window) = self.window.as_ref().map(|w| w.id()) else {
            return;
        };
        let state = if down {
            ElementState::Pressed
        } else {
            ElementState::Released
        };
        let mouse = match name.as_str() {
            "key.mouse.left" => Some(MouseButton::Left),
            "key.mouse.right" => Some(MouseButton::Right),
            "key.mouse.middle" => Some(MouseButton::Middle),
            _ => None,
        };
        if let Some(button) = mouse {
            let event = WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state,
                button,
            };
            self.user_window_event(event_loop, window, event);
            return;
        }
        let Some(code) = horizon_client::keybinds::key_code(&name) else {
            return;
        };
        #[cfg(target_os = "ios")]
        {
            use winit::keyboard::{Key, NativeKey};
            let logical = match horizon_ios_input::named_key(code) {
                Some(named) => Key::Named(named),
                None => Key::Unidentified(NativeKey::Unidentified),
            };
            let event = WindowEvent::KeyboardInput {
                device_id: DeviceId::dummy(),
                event: horizon_ios_input::key_event(code, logical, None, state, false),
                is_synthetic: false,
            };
            self.user_window_event(event_loop, window, event);
        }
        #[cfg(not(target_os = "ios"))]
        let _ = (code, state, event_loop);
    }

    /// What the buttons asked for since the last call: their keys, and the shortcuts' keys.
    fn touch_buttons_keys(&mut self, event_loop: &dyn EventLoopCtl) {
        let Some(buttons) = &mut self.touch.buttons else {
            return;
        };
        let mut keys: Vec<(String, bool)> = buttons.key_events();
        for (asked, name) in [
            (&mut buttons.chat_requested, "action:key.chat"),
            (&mut buttons.debug_requested, "key.keyboard.f3"),
            (
                &mut buttons.perspective_requested,
                "action:key.togglePerspective",
            ),
        ] {
            if std::mem::take(asked) {
                keys.push((name.to_owned(), true));
                keys.push((name.to_owned(), false));
            }
        }
        // Settings: the game's menu, which has its options.
        let settings = std::mem::take(&mut buttons.settings_requested);
        buttons.clicked = false;
        for (name, down) in keys {
            self.touch_key(event_loop, &name, down);
        }
        if settings {
            self.touch_tap(Button::Start);
        }
    }

    /// The window's safe area, once a frame, for the menus and HUD (`hud::set_safe_area`).
    pub(super) fn sync_safe_area(&self) {
        if let (Some(window), Some(r)) = (&self.window, &self.renderer) {
            horizon_client::hud::set_safe_area(
                [r.config.width, r.config.height],
                horizon_client::window::safe_insets(window),
            );
        }
    }

    pub(super) fn touch_playing(&self) -> bool {
        self.title.is_none() && self.screen == Screen::Playing && self.player.is_some()
    }

    /// A screen open over play (the inventory, a container, the pause menu, chat).
    pub(super) fn touch_screen_over_play(&self) -> bool {
        self.title.is_none() && self.screen != Screen::Playing && self.player.is_some()
    }

    pub(super) fn touch_scale(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, |r| {
            HudBatch::new([r.config.width, r.config.height]).scale
        })
    }

    /// Window pixels per point.
    fn touch_points(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    pub(super) fn touch_press(&mut self, button: Button, down: bool) {
        let state = self.pad.touch_mut();
        match button {
            Button::LeftTrigger => state.triggers[0] = if down { 1.0 } else { 0.0 },
            Button::RightTrigger => state.triggers[1] = if down { 1.0 } else { 0.0 },
            _ => state.set(button, down),
        }
    }

    pub(super) fn touch_tap(&mut self, button: Button) {
        self.touch_press(button, true);
        self.touch.taps.push(button);
    }

    /// Lets go of everything the touch controls hold on the virtual pad.
    fn touch_release_all(&mut self) {
        for button in [
            Button::LeftStick,
            Button::RightStick,
            Button::LeftTrigger,
            Button::RightTrigger,
            Button::South,
        ] {
            self.touch_press(button, false);
        }
        self.pad.touch_mut().left = [0.0; 2];
        self.touch.taps.clear();
        self.touch.releasing.clear();
        self.touch.aim = None;
        for finger in self.touch.fingers.values_mut() {
            if !matches!(finger.kind, Kind::Mouse | Kind::Scroll) {
                finger.kind = Kind::Ignored;
            }
        }
    }

    /// The pick's ray when a touch aims it: through the finger's point of the game, else none
    /// (the view's centre).
    pub(super) fn touch_aim(&self, yaw: f32, pitch: f32) -> Option<[f64; 3]> {
        let (point, until) = self.touch.aim?;
        if until.is_some_and(|until| Instant::now() > until) {
            return None;
        }
        // The point is the HUD's, in the safe area; the view fills the whole surface.
        let surface = self
            .renderer
            .as_ref()
            .map_or([1, 1], |r| [r.config.width.max(1), r.config.height.max(1)]);
        let [left, top, ..] = horizon_client::hud::safe_insets(surface);
        let scale = self.touch_scale();
        let (w, h) = (surface[0] as f32, surface[1] as f32);
        let px = [
            point[0] * scale + left as f32,
            point[1] * scale + top as f32,
        ];
        // Normalised device coordinates of the point, y up.
        let nx = f64::from(px[0] / w * 2.0 - 1.0);
        let ny = f64::from(1.0 - px[1] / h * 2.0);
        let fov = if self.touch.fov > 1.0 {
            self.touch.fov
        } else {
            70.0
        };
        let half = (f64::from(fov) / 2.0).to_radians().tan();
        let aspect = f64::from(w / h);
        let forward = super::view_vector(yaw, pitch);
        // Right of the view (yaw 0 faces +Z, whose right is -X), then up = right × forward.
        let (ys, yc) = f64::from(yaw).to_radians().sin_cos();
        let right = [-yc, 0.0, -ys];
        let up = [
            right[1] * forward[2] - right[2] * forward[1],
            right[2] * forward[0] - right[0] * forward[2],
            right[0] * forward[1] - right[1] * forward[0],
        ];
        let (sx, sy) = (nx * half * aspect, ny * half);
        let dir: [f64; 3] = std::array::from_fn(|i| forward[i] + right[i] * sx + up[i] * sy);
        let len = dir.iter().map(|v| v * v).sum::<f64>().sqrt();
        Some(dir.map(|v| v / len))
    }

    /// winit's touch event.
    pub(super) fn touch_event(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        id: WindowId,
        touch: Touch,
    ) {
        if is_pointer(&touch) {
            return;
        }
        if touch.phase == TouchPhase::Started {
            self.hardware.touched();
        }
        self.touch.seen = true;
        self.last_input = Instant::now();
        let scale = self.touch_scale();
        let points = self.touch_points();
        let gui = [
            touch.location.x as f32 / scale,
            touch.location.y as f32 / scale,
        ];
        let at = [touch.location.x / points, touch.location.y / points];
        let now = Instant::now();
        if touch.phase == TouchPhase::Started {
            let (w, h) = self.gui_size();
            // iPhone Duo upright: the deck's sides are the pads (its card is SwiftUI's, which
            // takes its own touches), and the game's whole view looks. Elsewhere the thumb walks
            // from the lower left corner and the rest of the view looks.
            let zone = self.deck_zone(&touch);
            let on_deck = zone.is_some();
            let side = match zone {
                Some(Some(side)) => side,
                Some(None) => Side::Act,
                None if self.deck_up() => Side::Act,
                None if gui[0] < w * WALK_WIDTH && gui[1] > h * (1.0 - WALK_HEIGHT) => Side::Walk,
                None => Side::Act,
            };
            let kind = if zone == Some(None) || (on_deck && !self.touch_playing()) {
                Kind::Ignored
            } else if self.touch.buttons.is_some() && !on_deck && self.touch_playing() {
                match self.touch_land_in_play(gui, side) {
                    Kind::Hotbar => Kind::Hotbar,
                    _ => Kind::Buttons,
                }
            } else if on_deck {
                self.touch_land_on_pad(side)
            } else if self.touch_playing() {
                self.touch_land_in_play(gui, side)
            } else if self
                .touch
                .fingers
                .values()
                .any(|f| matches!(f.kind, Kind::Mouse))
            {
                Kind::Scroll
            } else {
                Kind::Mouse
            };
            if matches!(kind, Kind::Mouse) {
                // A tap on the text field brings the keyboard up; anywhere else puts it away.
                let (x, y) = (touch.location.x as i32, touch.location.y as i32);
                self.touch.keyboard = self.touch.text_area.is_some_and(|[ax, ay, aw, ah]| {
                    let slop = 40;
                    x >= ax - slop && x < ax + aw + slop && y >= ay - slop && y < ay + ah + slop
                });
            }
            self.touch.fingers.insert(
                touch.id,
                Finger {
                    side,
                    on_deck,
                    kind,
                    start: at,
                    at,
                    at_gui: gui,
                    since: now,
                },
            );
        }
        let Some(mut finger) = self.touch.fingers.remove(&touch.id) else {
            return;
        };
        let ended = matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled);
        let last = finger.at;
        finger.at = at;
        finger.at_gui = gui;
        match finger.kind {
            Kind::Mouse => {
                for event in as_mouse(touch) {
                    self.handle_window_event(
                        event_loop,
                        id,
                        event,
                        super::input_trace::InputOrigin::User,
                    );
                }
                if ended
                    && finger.since.elapsed() < TAP
                    && finger.travelled() <= TAP_SLOP
                    && self.touch_tapped_outside_menu()
                {
                    self.touch_tap(Button::East);
                }
            }
            Kind::Scroll => {
                let dy = (at[1] - last[1]) / 12.0;
                if dy != 0.0 {
                    self.handle_window_event(
                        event_loop,
                        id,
                        WindowEvent::MouseWheel {
                            device_id: DeviceId::dummy(),
                            delta: MouseScrollDelta::LineDelta(0.0, dy as f32),
                            phase: TouchPhase::Moved,
                        },
                        super::input_trace::InputOrigin::User,
                    );
                }
            }
            Kind::Stick => {
                let mut v = [
                    (at[0] - finger.start[0]) / STICK_REACH,
                    -(at[1] - finger.start[1]) / STICK_REACH,
                ];
                let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
                if len > 1.0 {
                    v = [v[0] / len, v[1] / len];
                }
                // Pushed to the edge, mostly forward: sprint, as the stick's click does.
                let sprint = !ended && len >= SPRINT_AT && v[1] > 0.5;
                self.touch_press(Button::LeftStick, sprint);
                self.pad.touch_mut().left = if ended {
                    [0.0; 2]
                } else {
                    [v[0] as f32, v[1] as f32]
                };
                if ended && finger.since.elapsed() < TAP && finger.travelled() <= TAP_SLOP {
                    self.touch_tapped(&finger);
                }
            }
            Kind::Pending => self.touch_pending(&mut finger, ended),
            Kind::Buttons => {
                // The buttons are laid out in the safe area, as the HUD that draws them.
                let size = self.renderer.as_ref().map_or([1, 1], |r| {
                    horizon_client::hud::safe_size([r.config.width, r.config.height])
                        .map(|v| v.max(1))
                });
                let point = [
                    touch.location.x as f32 / size[0] as f32,
                    touch.location.y as f32 / size[1] as f32,
                ];
                let started = touch.phase == TouchPhase::Started;
                if let Some(buttons) = &mut self.touch.buttons {
                    buttons.viewport(size[0], size[1]);
                    let turn = buttons.update(touch.id, point, started, ended);
                    let [yaw, pitch] = horizon_client::touch::camera_delta(
                        turn,
                        size[0],
                        size[1],
                        &buttons.settings,
                    );
                    self.touch_look_degrees(yaw, pitch);
                }
                self.touch_buttons_keys(event_loop);
            }
            Kind::Look => self.touch_look(at[0] - last[0], at[1] - last[1]),
            Kind::Jump if ended => self.touch_press(Button::South, false),
            Kind::Crouch if ended => self.touch_press(Button::RightStick, false),
            Kind::Mine => {
                if ended {
                    self.touch_press(Button::RightTrigger, false);
                    self.touch.aim = None;
                } else if !finger.on_deck {
                    // Mining follows the finger.
                    self.touch.aim = Some((gui, None));
                }
            }
            _ => {}
        }
        if !ended {
            self.touch.fingers.insert(touch.id, finger);
        }
    }

    /// A finger on the acting half that is not yet anything: a tap when it lifts, a swipe up
    /// (jump) or down (crouch) when it moves more vertically than across, else a look.
    fn touch_pending(&mut self, finger: &mut Finger, ended: bool) {
        let travelled = finger.travelled();
        if ended {
            if finger.since.elapsed() < TAP && travelled <= TAP_SLOP {
                self.touch_tapped(finger);
            }
            return;
        }
        if travelled <= TAP_SLOP {
            return;
        }
        let (dx, dy) = (
            finger.at[0] - finger.start[0],
            finger.at[1] - finger.start[1],
        );
        if dy.abs() > dx.abs() {
            // Up or down: a swipe once it has gone far enough.
            if dy.abs() >= SWIPE_DISTANCE {
                let (button, kind) = if dy < 0.0 {
                    (Button::South, Kind::Jump)
                } else {
                    (Button::RightStick, Kind::Crouch)
                };
                self.touch_press(button, true);
                finger.kind = kind;
            }
        } else {
            // Across: a look, from here on, on the game's view or the deck's pad alike. The
            // way it came counts past the tap's slop: a quick swipe arrives in few moves, and
            // dropping them all would barely turn the view; the slop itself is left out, so the
            // view doesn't jump as the look starts.
            finger.kind = Kind::Look;
            let past = (travelled - TAP_SLOP).max(0.0) / travelled;
            self.touch_look(dx * past, dy * past);
        }
    }

    /// What a finger landing at `gui` does in play.
    fn touch_land_in_play(&mut self, gui: [f32; 2], side: Side) -> Kind {
        let (w, h) = self.gui_size();
        // The hotbar: 182 by 22 GUI pixels, centred at the bottom (Gui.extractItemHotbar).
        let left = (w / 2.0 - 91.0).floor();
        if gui[1] >= h - 22.0 && gui[0] >= left && gui[0] < left + 182.0 {
            let slot = (((gui[0] - left - 1.0) / 20.0) as usize).min(8);
            self.hotbar_presses.push(slot);
            return Kind::Hotbar;
        }
        let stick_taken = self
            .touch
            .fingers
            .values()
            .any(|f| matches!(f.kind, Kind::Stick));
        match side {
            Side::Walk if !stick_taken => Kind::Stick,
            Side::Walk => Kind::Ignored,
            Side::Act => Kind::Pending,
        }
    }

    /// What a finger landing on one of the deck's pads does in play.
    fn touch_land_on_pad(&mut self, side: Side) -> Kind {
        let stick_taken = self
            .touch
            .fingers
            .values()
            .any(|f| matches!(f.kind, Kind::Stick));
        match side {
            Side::Walk if !stick_taken => Kind::Stick,
            Side::Walk => Kind::Ignored,
            Side::Act => Kind::Pending,
        }
    }

    /// Turns the view by degrees, as the mouse would at the default sensitivity.
    fn touch_look_degrees(&mut self, yaw: f32, pitch: f32) {
        self.touch_look(
            f64::from(yaw / DEGREES_PER_COUNT) / LOOK_SPEED,
            f64::from(pitch / DEGREES_PER_COUNT) / LOOK_SPEED,
        );
    }

    fn touch_look(&mut self, dx: f64, dy: f64) {
        if dx != 0.0 || dy != 0.0 {
            self.mouse
                .on_move(0.0, 0.0, dx * LOOK_SPEED, dy * LOOK_SPEED, true, true);
        }
    }

    /// A tap on one half: the inventory (once the double tap's window has passed), a double
    /// tap's use, or half of the pause's double tap with both hands.
    fn touch_tapped(&mut self, finger: &Finger) {
        let now = Instant::now();
        let side = finger.side as usize;
        let other = 1 - side;
        // The other hand tapped at the same time: a tap with both hands, not a jump.
        if self.touch.last_tap[other].is_some_and(|(at, _)| now - at <= BOTH_SIDES) {
            self.touch.last_tap = [None, None];
            self.touch.inventory_at = None;
            if self
                .touch
                .last_both
                .is_some_and(|at| now - at <= DOUBLE_TAP + BOTH_SIDES)
            {
                self.touch.last_both = None;
                self.touch_tap(Button::Start);
            } else {
                self.touch.last_both = Some(now);
            }
            return;
        }
        let previous = self.touch.last_tap[side].replace((now, finger.at));
        if finger.side == Side::Walk {
            return;
        }
        if previous.is_some_and(|(at, p)| now - at <= DOUBLE_TAP && distance(p, finger.at) < 40.0) {
            // A double tap: use or place at the finger.
            self.touch.last_tap[side] = None;
            self.touch.inventory_at = None;
            self.touch.aim = (!finger.on_deck).then_some((finger.at_gui, Some(now + AIM_LINGER)));
            self.touch_tap(Button::LeftTrigger);
        } else {
            // A single tap opens the inventory, unless a second tap makes it a double tap.
            self.touch.inventory_at = Some(now + DOUBLE_TAP);
        }
    }

    /// The tap just given to a menu opened in play fell outside it: on no slot or panel of a
    /// container screen (`hasClickedOutside`) with nothing carried, or on no widget of another.
    fn touch_tapped_outside_menu(&self) -> bool {
        if !self.touch_screen_over_play() || self.carried.is_some() {
            return false;
        }
        // Screens that type take taps for their text.
        if matches!(
            self.screen,
            Screen::Chat | Screen::CommandBlock | Screen::Options | Screen::Social
        ) || self.sign_edit.is_some()
            || self.book.is_some()
        {
            return false;
        }
        let Some(r) = &self.renderer else {
            return false;
        };
        let batch = HudBatch::new([r.config.width, r.config.height]);
        if matches!(self.screen, Screen::Inventory | Screen::Creative) {
            return self.clicked_outside(&batch);
        }
        let (x, y) = self.cursor;
        let grow = 4.0;
        !horizon_client::widgets::last().iter().any(|w| {
            x >= w.x - grow && x < w.x + w.w + grow && y >= w.y - grow && y < w.y + w.h + grow
        })
    }

    /// Whether the input method (the on-screen keyboard) may show while a text field has focus:
    /// on iOS only once the field was tapped.
    pub(super) fn touch_keyboard_wanted(&mut self, focused: bool) -> bool {
        if !focused {
            self.touch.keyboard = false;
            self.touch.text_area = None;
        }
        // A hardware keyboard leaves the screen free: UIKit shows no keyboard, and the field
        // needs its text input.
        !cfg!(target_os = "ios") || self.touch.keyboard || horizon_ios_input::keyboard_connected()
    }

    pub(super) fn touch_keyboard_done(&mut self) {
        self.touch.keyboard = false;
    }

    pub(super) fn touch_text_area(&mut self, area: [i32; 4]) {
        self.touch.text_area = Some(area);
    }

    /// Once a frame, before the pads are read: taps let go, a still press starts mining, and
    /// leaving play lets go of everything.
    pub(super) fn touch_frame(&mut self, event_loop: &dyn EventLoopCtl) {
        self.touch_buttons_load();
        for button in std::mem::take(&mut self.touch.releasing) {
            self.touch_press(button, false);
        }
        self.touch.releasing = std::mem::take(&mut self.touch.taps);
        let playing = self.touch_playing();
        if self.touch.was_playing && !playing {
            self.touch_release_all();
            if let Some(buttons) = &mut self.touch.buttons {
                buttons.clear();
            }
            self.touch_buttons_keys(event_loop);
        }
        self.touch.was_playing = playing;
        if playing && self.touch.buttons.is_some() {
            self.touch_buttons_frame();
            return;
        }
        if !playing {
            self.touch.inventory_at = None;
            return;
        }
        if self
            .touch
            .inventory_at
            .is_some_and(|at| Instant::now() >= at)
        {
            self.touch.inventory_at = None;
            self.touch_tap(Button::North);
        }
        let mut mine = None;
        for finger in self.touch.fingers.values_mut() {
            if matches!(finger.kind, Kind::Pending)
                && finger.since.elapsed() >= HOLD
                // A finger already on its way (a swipe deciding) is no hold.
                && finger.travelled() <= TAP_SLOP
            {
                finger.kind = Kind::Mine;
                mine = Some((!finger.on_deck).then_some(finger.at_gui));
            }
        }
        if let Some(at) = mine {
            self.touch.aim = at.map(|at| (at, None));
            self.touch_press(Button::RightTrigger, true);
        }
    }

    /// The Buttons scheme, once a frame in play: the virtual pad holds what the fingers hold, and
    /// a tap or a hold on the world aims where it landed.
    fn touch_buttons_frame(&mut self) {
        let flying = self.player.as_ref().is_some_and(|p| p.flying);
        let (w, h) = self.gui_size();
        let Some(buttons) = &mut self.touch.buttons else {
            return;
        };
        buttons.vertical_movement = flying;
        let state = buttons.poll_state();
        let aim = buttons.aim;
        let tap = buttons.take_tap();
        *self.pad.touch_mut() = state;
        if let Some([x, y]) = aim {
            self.touch.aim = Some(([x * w, y * h], Some(Instant::now() + AIM_LINGER)));
        }
        match tap {
            Some(true) => self.touch_tap(Button::LeftTrigger),
            Some(false) => self.touch_tap(Button::RightTrigger),
            None => {}
        }
    }

    /// The Buttons scheme's controls over play; the gestures need no buttons or stick to see.
    pub(super) fn draw_touch_controls(&self, batch: &mut HudBatch) {
        let Some(buttons) = &self.touch.buttons else {
            return;
        };
        if !self.touch_playing() || self.hardware.active() {
            return;
        }
        horizon_client::touch::draw::controls(batch, &self.gui.atlas, buttons, &|what| {
            horizon_client::touch::draw::name(buttons, what)
        });
    }
}
