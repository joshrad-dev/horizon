//! Drawing the on-screen controls ([`super::TouchControls`]) on the HUD: a translucent tile per
//! button with its label, a ring and a knob for the joystick. The layout editor draws them the
//! same way, with its selection on top.
use super::{Control, Drawn, Mode, Shown, TouchControls};
use crate::hud::{HudAtlas, HudBatch};

/// How many sides a drawn circle has.
const SIDES: usize = 32;

/// `color` with its alpha multiplied by `alpha` (0..1).
pub fn faded(color: u32, alpha: f32) -> u32 {
    let a = ((color >> 24) as f32 * alpha.clamp(0.0, 1.0)).round() as u32;
    (a << 24) | (color & 0x00ff_ffff)
}

/// A ring from radius `inner` to `outer` around `c` (GUI pixels); `inner` 0 fills a disc.
pub fn ring(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    c: [f32; 2],
    inner: f32,
    outer: f32,
    color: u32,
) {
    let at = |i: usize, r: f32| {
        let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        [c[0] + a.cos() * r, c[1] + a.sin() * r]
    };
    for i in 0..SIDES {
        let quad = [
            at(i, outer),
            at(i + 1, outer),
            at(i + 1, inner),
            at(i, inner),
        ];
        batch.fill_quad(atlas, quad, color);
    }
}

/// A frame `t` pixels thick inside `[x, y, w, h]`.
pub fn frame(batch: &mut HudBatch, atlas: &HudAtlas, [x, y, w, h]: [f32; 4], t: f32, color: u32) {
    batch.fill(atlas, x, y, w, t, color);
    batch.fill(atlas, x, y + h - t, w, t, color);
    batch.fill(atlas, x, y + t, t, h - 2.0 * t, color);
    batch.fill(atlas, x + w - t, y + t, t, h - 2.0 * t, color);
}

/// `text` centred on `c`, shrunk to fit `width`.
pub fn label(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    text: &str,
    c: [f32; 2],
    width: f32,
    color: u32,
) {
    let w = HudBatch::text_width(atlas, text);
    if w <= 0.0 {
        return;
    }
    let start = batch.vertices.len();
    batch.text(atlas, text, c[0] - w / 2.0, c[1] - 4.0, color, true);
    let fit = (width / w).min(1.0);
    if fit < 1.0 {
        batch.scale_since(start, c[0], c[1], fit, fit);
    }
}

/// A control's rectangle in GUI pixels.
pub fn gui_rect(batch: &HudBatch, r: [f32; 4]) -> [f32; 4] {
    [
        r[0] * batch.width,
        r[1] * batch.height,
        r[2] * batch.width,
        r[3] * batch.height,
    ]
}

/// One control's tile: its label (`name`), dimmed by its opacity, lit while pressed.
pub fn tile(batch: &mut HudBatch, atlas: &HudAtlas, d: &Drawn, name: &str) {
    let [x, y, w, h] = gui_rect(batch, d.rect);
    let back = if d.pressed { 0xa0ff_ffff } else { 0x9010_1418 };
    let text = if d.pressed { 0xff10_1418 } else { 0xffff_ffff };
    if d.what == Shown::Control(Control::Move) {
        return;
    }
    batch.fill(
        atlas,
        x + 1.0,
        y + 1.0,
        w - 2.0,
        h - 2.0,
        faded(back, d.opacity),
    );
    frame(
        batch,
        atlas,
        [x, y, w, h],
        1.0,
        faded(0xc0ff_ffff, d.opacity),
    );
    label(
        batch,
        atlas,
        name,
        [x + w / 2.0, y + h / 2.0],
        w - 4.0,
        faded(text, d.opacity.max(0.5)),
    );
}

/// The joystick at rest at `rect`, or held (`stick`: centre, radius, knob).
pub fn joystick(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    rect: [f32; 4],
    opacity: f32,
    held: Option<([f32; 2], f32, [f32; 2])>,
    dpad: bool,
) {
    let (sx, sy) = (batch.width, batch.height);
    let (centre, radius, knob) = match held {
        Some((c, r, k)) => ([c[0] * sx, c[1] * sy], r * sy, [k[0] * sx, k[1] * sy]),
        None => {
            let [x, y, w, h] = gui_rect(batch, rect);
            let c = [x + w / 2.0, y + h / 2.0];
            (c, h / 2.0, c)
        }
    };
    if dpad {
        // Four arms round a centre, as a plus.
        let arm = radius * 2.0 / 3.0;
        let half = radius / 3.0;
        let back = faded(0x9010_1418, opacity);
        batch.fill(
            atlas,
            centre[0] - half,
            centre[1] - radius,
            half * 2.0,
            radius * 2.0,
            back,
        );
        batch.fill(
            atlas,
            centre[0] - radius,
            centre[1] - half,
            radius - half,
            half * 2.0,
            back,
        );
        batch.fill(
            atlas,
            centre[0] + half,
            centre[1] - half,
            radius - half,
            half * 2.0,
            back,
        );
        let lit = faded(0x60ff_ffff, opacity);
        if held.is_some() {
            let d = [knob[0] - centre[0], knob[1] - centre[1]];
            let step = |v: f32| {
                if v.abs() < radius / 3.0 {
                    0.0
                } else {
                    v.signum()
                }
            };
            let (ax, ay) = (step(d[0]), step(d[1]));
            batch.fill(
                atlas,
                centre[0] + ax * arm - half,
                centre[1] + ay * arm - half,
                half * 2.0,
                half * 2.0,
                lit,
            );
        }
        return;
    }
    ring(
        batch,
        atlas,
        centre,
        0.0,
        radius,
        faded(0x6010_1418, opacity),
    );
    ring(
        batch,
        atlas,
        centre,
        radius - 1.5,
        radius,
        faded(0xc0ff_ffff, opacity),
    );
    let knob_color = if held.is_some() {
        0xe0ff_ffff
    } else {
        0xa0ff_ffff
    };
    ring(
        batch,
        atlas,
        knob,
        0.0,
        radius * 0.42,
        faded(knob_color, opacity.max(0.4)),
    );
}

/// Every control that shows, as the player sees them in play. `name` labels each.
pub fn controls(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    touch: &TouchControls,
    name: &dyn Fn(Shown) -> String,
) {
    let dpad = touch.settings.mode == Mode::Dpad;
    let held = touch.stick();
    let mut stick_drawn = false;
    for d in touch.drawn() {
        if d.what == Shown::Control(Control::Move) {
            joystick(batch, atlas, d.rect, d.opacity, held, dpad);
            stick_drawn = true;
        } else {
            tile(batch, atlas, &d, &name(d.what));
        }
    }
    // A dynamic joystick shows where the thumb landed.
    if !stick_drawn && held.is_some() {
        let rect = touch.control_rect(Control::Move);
        let opacity = touch.settings.placement(Control::Move).opacity;
        joystick(batch, atlas, rect, opacity, held, dpad);
    }
}

/// A control's translation key, for its label.
pub fn control_key(c: Control) -> &'static str {
    match c {
        Control::Move => "horizon.touch.move",
        Control::Sprint => "horizon.touch.sprint",
        Control::Jump => "horizon.touch.jump",
        Control::Sneak => "horizon.touch.sneak",
        Control::Attack => "horizon.touch.attack",
        Control::Use => "horizon.touch.use",
        Control::Chat => "horizon.touch.chat",
        Control::Pause => "horizon.touch.pause",
        Control::Settings => "horizon.touch.settings",
        Control::Inventory => "horizon.touch.inventory",
        Control::Debug => "horizon.touch.debug",
        Control::Perspective => "horizon.touch.perspective",
        Control::Offhand => "horizon.touch.offhand",
        Control::Shield => "horizon.touch.shield",
    }
}

/// What a control says: a built-in one's name, a player's button's label or else its key.
pub fn name(touch: &TouchControls, what: Shown) -> String {
    match what {
        Shown::Control(c) => crate::lang::get(control_key(c)),
        Shown::Custom(i) => touch.settings.custom.get(i).map_or_else(String::new, |b| {
            if b.label.is_empty() {
                horizon_client_core::key_display::display_name(&b.key).plain()
            } else {
                b.label.clone()
            }
        }),
    }
}
