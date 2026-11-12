//! The on-screen controls' layout editor, which the launcher opens (Controls > Edit layout): every
//! control at its place, the hidden ones faint. A touch picks one up and drags it; the toolbar
//! resizes the picked one, changes its opacity, hides or shows it, and for the player's own
//! buttons picks the key, makes it a toggle or deletes it. It also adds buttons, switches the
//! scheme and the hand, and resets the layout. Positions are in GUI pixels in and out; the layout
//! keeps fractions of the surface ([`super::layout`]).
use super::draw;
use super::layout::{self, CustomButton, HIDDEN, MAX_CUSTOM, SHOWN};
use super::{Control, Mode, Settings, Shown, TouchControls};
use crate::hud::{HudAtlas, HudBatch};

/// What the toolbar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Add,
    Smaller,
    Bigger,
    Fade,
    Hide,
    Key,
    Toggle,
    Delete,
    Scheme,
    Hand,
    Reset,
    Done,
}

impl Tool {
    const ALL: [Self; 12] = [
        Self::Add,
        Self::Smaller,
        Self::Bigger,
        Self::Fade,
        Self::Hide,
        Self::Key,
        Self::Toggle,
        Self::Delete,
        Self::Scheme,
        Self::Hand,
        Self::Reset,
        Self::Done,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Add => "horizon.touch.editor.add",
            Self::Smaller => "horizon.touch.editor.smaller",
            Self::Bigger => "horizon.touch.editor.bigger",
            Self::Fade => "horizon.touch.editor.opacity",
            Self::Hide => "horizon.touch.editor.hide",
            Self::Key => "horizon.touch.editor.key",
            Self::Toggle => "horizon.touch.editor.toggle",
            Self::Delete => "horizon.touch.editor.delete",
            Self::Scheme => "horizon.touch.editor.scheme",
            Self::Hand => "horizon.touch.editor.hand",
            Self::Reset => "horizon.touch.editor.reset",
            Self::Done => "horizon.touch.editor.done",
        }
    }
}

/// The keys a button may hold, as `options.txt` names them.
pub const KEYS: &[&str] = &[
    "key.keyboard.q",
    "key.keyboard.e",
    "key.keyboard.r",
    "key.keyboard.f",
    "key.keyboard.c",
    "key.keyboard.x",
    "key.keyboard.z",
    "key.keyboard.v",
    "key.keyboard.b",
    "key.keyboard.g",
    "key.keyboard.h",
    "key.keyboard.j",
    "key.keyboard.k",
    "key.keyboard.l",
    "key.keyboard.m",
    "key.keyboard.n",
    "key.keyboard.o",
    "key.keyboard.p",
    "key.keyboard.t",
    "key.keyboard.u",
    "key.keyboard.y",
    "key.keyboard.i",
    "key.keyboard.w",
    "key.keyboard.a",
    "key.keyboard.s",
    "key.keyboard.d",
    "key.keyboard.1",
    "key.keyboard.2",
    "key.keyboard.3",
    "key.keyboard.4",
    "key.keyboard.5",
    "key.keyboard.6",
    "key.keyboard.7",
    "key.keyboard.8",
    "key.keyboard.9",
    "key.keyboard.0",
    "key.keyboard.f1",
    "key.keyboard.f2",
    "key.keyboard.f3",
    "key.keyboard.f5",
    "key.keyboard.f11",
    "key.keyboard.escape",
    "key.keyboard.tab",
    "key.keyboard.space",
    "key.keyboard.enter",
    "key.keyboard.backspace",
    "key.keyboard.left.shift",
    "key.keyboard.left.control",
    "key.keyboard.left.alt",
    "key.keyboard.up",
    "key.keyboard.down",
    "key.keyboard.left",
    "key.keyboard.right",
    "key.keyboard.slash",
];

/// What a touch in the editor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Control(Control),
    Custom(usize),
}

/// Toolbar buttons are this tall, in GUI pixels.
const TOOL_HEIGHT: f32 = 16.0;
const KEY_CELL: [f32; 2] = [34.0, 16.0];

pub struct Editor {
    /// The layout being edited, inside the controls that place it on this surface.
    pub controls: TouchControls,
    pub selected: Option<Target>,
    /// The finger dragging the selection, and where on it the finger holds it (fractions).
    drag: Option<(u64, [f32; 2])>,
    /// The key grid shows, for the player's button being set.
    pub picking_key: bool,
    /// Done was pressed: the caller saves [`Self::settings`] and closes the editor.
    pub done: bool,
    /// The GUI's size the last frame was drawn at.
    gui: [f32; 2],
    tools: Vec<(Tool, [f32; 4])>,
    keys: Vec<(&'static str, [f32; 4])>,
}

fn inside([x, y, w, h]: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= x && p[0] < x + w && p[1] >= y && p[1] < y + h
}

impl Editor {
    pub fn new(settings: Settings) -> Self {
        Self {
            controls: TouchControls::new(settings),
            selected: None,
            drag: None,
            picking_key: false,
            done: false,
            gui: [0.0; 2],
            tools: Vec::new(),
            keys: Vec::new(),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.controls.settings
    }

    fn settings_mut(&mut self) -> &mut Settings {
        &mut self.controls.settings
    }

    /// The saved placement of `target`, as stored (unmirrored).
    fn placement_mut(&mut self, target: Target) -> Option<&mut layout::Placement> {
        let s = &mut self.controls.settings;
        match target {
            Target::Control(c) => {
                let slot = s.slot(c);
                Some(&mut s.layouts[s.mode.index()][slot])
            }
            Target::Custom(i) => s.custom.get_mut(i).map(|b| &mut b.placement),
        }
    }

    fn rect(&self, target: Target) -> [f32; 4] {
        match target {
            Target::Control(c) => self.controls.control_rect(c),
            Target::Custom(i) => self
                .controls
                .custom_placement(i)
                .rect(self.gui[0] / self.gui[1].max(1.0)),
        }
    }

    /// What is under `p` (fractions of the surface): the player's buttons first, as drawn last.
    fn target_at(&self, p: [f32; 2]) -> Option<Target> {
        (0..self.controls.settings.custom.len())
            .rev()
            .map(Target::Custom)
            .chain(Control::ALL.into_iter().rev().map(Target::Control))
            .find(|&t| layout::contains(self.rect(t), p))
    }

    fn usable(&self, tool: Tool) -> bool {
        match tool {
            Tool::Smaller | Tool::Bigger | Tool::Fade => self.selected.is_some(),
            Tool::Hide => matches!(self.selected, Some(Target::Control(_))),
            Tool::Key | Tool::Toggle | Tool::Delete => {
                matches!(self.selected, Some(Target::Custom(_)))
            }
            Tool::Add => self.controls.settings.custom.len() < MAX_CUSTOM,
            _ => true,
        }
    }

    /// A finger landing (`started`), moving, or lifting (`ended`) at `gui` (GUI pixels).
    pub fn touch(&mut self, id: u64, gui: [f32; 2], started: bool, ended: bool) {
        if self.gui[0] <= 0.0 {
            return;
        }
        let p = [gui[0] / self.gui[0], gui[1] / self.gui[1]];
        if started {
            if self.picking_key {
                let picked = self.keys.iter().find(|(_, r)| inside(*r, gui)).map(|k| k.0);
                if let (Some(key), Some(Target::Custom(i))) = (picked, self.selected)
                    && let Some(button) = self.settings_mut().custom.get_mut(i)
                {
                    button.key = key.to_owned();
                }
                self.picking_key = false;
                return;
            }
            if let Some(tool) = self
                .tools
                .iter()
                .find(|(_, r)| inside(*r, gui))
                .map(|t| t.0)
            {
                if self.usable(tool) {
                    self.apply(tool);
                }
                return;
            }
            self.selected = self.target_at(p);
            if let Some(target) = self.selected {
                let r = self.rect(target);
                let centre = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
                self.drag = Some((id, [p[0] - centre[0], p[1] - centre[1]]));
            }
            return;
        }
        let Some((finger, offset)) = self.drag else {
            return;
        };
        if finger != id {
            return;
        }
        if ended {
            self.drag = None;
            return;
        }
        let left_handed = self.controls.settings.left_handed;
        let Some(target) = self.selected else {
            return;
        };
        if let Some(placement) = self.placement_mut(target) {
            let mut x = (p[0] - offset[0]).clamp(0.025, 0.975);
            if left_handed {
                x = 1.0 - x;
            }
            placement.center = [x, (p[1] - offset[1]).clamp(0.025, 0.975)];
        }
    }

    pub fn apply(&mut self, tool: Tool) {
        let selected = self.selected;
        match tool {
            Tool::Add => {
                let s = self.settings_mut();
                s.custom.push(CustomButton::default());
                self.selected = Some(Target::Custom(s.custom.len() - 1));
                self.picking_key = true;
            }
            Tool::Smaller | Tool::Bigger => {
                let factor = if tool == Tool::Bigger {
                    1.12
                } else {
                    1.0 / 1.12
                };
                if let Some(p) = selected.and_then(|t| self.placement_mut(t)) {
                    p.size = (p.size * factor).clamp(0.045, 0.45);
                }
            }
            Tool::Fade => {
                if let Some(p) = selected.and_then(|t| self.placement_mut(t)) {
                    const STEPS: [f32; 5] = [0.25, 0.45, 0.65, 0.85, 1.0];
                    let next = STEPS.iter().find(|&&o| o > p.opacity + 0.01);
                    p.opacity = *next.unwrap_or(&STEPS[0]);
                }
            }
            Tool::Hide => {
                if let Some(Target::Control(c)) = selected {
                    let shown = self.controls.shows(c);
                    self.settings_mut().button_visibility[c as usize] =
                        if shown { HIDDEN } else { SHOWN };
                }
            }
            Tool::Key => self.picking_key = true,
            Tool::Toggle => {
                if let Some(Target::Custom(i)) = selected
                    && let Some(b) = self.settings_mut().custom.get_mut(i)
                {
                    b.toggle = !b.toggle;
                }
            }
            Tool::Delete => {
                if let Some(Target::Custom(i)) = selected {
                    let s = self.settings_mut();
                    if i < s.custom.len() {
                        s.custom.remove(i);
                    }
                    self.selected = None;
                }
            }
            Tool::Scheme => {
                let s = self.settings_mut();
                let now = Mode::SCHEMES
                    .iter()
                    .position(|&(m, split, _)| m == s.mode && split == s.split_controls)
                    .unwrap_or(0);
                let (mode, split, _) = Mode::SCHEMES[(now + 1) % Mode::SCHEMES.len()];
                s.mode = mode;
                s.split_controls = split;
                self.selected = None;
            }
            Tool::Hand => {
                let s = self.settings_mut();
                s.left_handed = !s.left_handed;
            }
            Tool::Reset => {
                *self.settings_mut() = Settings::default();
                self.selected = None;
            }
            Tool::Done => {
                self.controls.settings.sanitize();
                self.done = true;
            }
        }
    }

    /// The scheme's translation key, for the toolbar's note.
    pub fn scheme_key(&self) -> &'static str {
        let s = &self.controls.settings;
        match (s.mode, s.split_controls) {
            (Mode::Crosshair, _) => "horizon.touch.scheme.crosshair",
            (Mode::Tap, _) => "horizon.touch.scheme.tap",
            (Mode::Dpad, false) => "horizon.touch.scheme.dpad",
            (Mode::Dpad, true) => "horizon.touch.scheme.dpad_crosshair",
        }
    }

    /// The editor over whatever is behind it: a dimmed screen, every control, the selection, the
    /// toolbar, and the key grid while it shows.
    pub fn draw(&mut self, batch: &mut HudBatch, atlas: &HudAtlas) {
        let (w, h) = (batch.width, batch.height);
        self.gui = [w, h];
        self.controls.viewport(w.max(1.0) as u32, h.max(1.0) as u32);
        batch.fill(atlas, 0.0, 0.0, w, h, 0xa000_0000);
        // Every built-in control, the hidden ones faint, then the player's.
        let dpad = self.controls.settings.mode == Mode::Dpad;
        for c in Control::ALL {
            let shown = self.controls.shows(c);
            let p = self.controls.settings.placement(c);
            let opacity = if shown { p.opacity.max(0.35) } else { 0.2 };
            let rect = self.controls.control_rect(c);
            if c == Control::Move {
                draw::joystick(batch, atlas, rect, opacity, None, dpad);
                continue;
            }
            let d = super::Drawn {
                what: Shown::Control(c),
                rect,
                opacity,
                pressed: false,
            };
            draw::tile(batch, atlas, &d, &draw::name(&self.controls, d.what));
        }
        for d in self.controls.drawn() {
            if let Shown::Custom(_) = d.what {
                let d = super::Drawn {
                    opacity: d.opacity.max(0.35),
                    pressed: false,
                    ..d
                };
                draw::tile(batch, atlas, &d, &draw::name(&self.controls, d.what));
            }
        }
        if let Some(target) = self.selected {
            let r = draw::gui_rect(batch, self.rect(target));
            draw::frame(
                batch,
                atlas,
                [r[0] - 2.0, r[1] - 2.0, r[2] + 4.0, r[3] + 4.0],
                2.0,
                0xffff_d040,
            );
        }
        self.draw_tools(batch, atlas);
        if self.picking_key {
            self.draw_keys(batch, atlas);
        } else {
            self.keys.clear();
        }
    }

    fn draw_tools(&mut self, batch: &mut HudBatch, atlas: &HudAtlas) {
        let w = batch.width;
        let labels: Vec<(Tool, String)> = Tool::ALL
            .into_iter()
            .map(|t| {
                let mut text = crate::lang::get(t.key());
                if t == Tool::Hide
                    && let Some(Target::Control(c)) = self.selected
                    && !self.controls.shows(c)
                {
                    text = crate::lang::get("horizon.touch.editor.show");
                }
                (t, text)
            })
            .collect();
        // As many rows as the width needs, centred.
        let widths: Vec<f32> = labels
            .iter()
            .map(|(_, text)| HudBatch::text_width(atlas, text) + 10.0)
            .collect();
        let gap = 3.0;
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        for (i, width) in widths.iter().enumerate() {
            if used + width > w - 16.0 && !rows.last().is_some_and(Vec::is_empty) {
                rows.push(Vec::new());
                used = 0.0;
            }
            used += width + gap;
            rows.last_mut().unwrap().push(i);
        }
        self.tools.clear();
        // Below the top row of shortcut buttons, in the middle the look gestures leave free.
        let mut y = (batch.height * 0.17).floor();
        for row in rows {
            let total: f32 = row.iter().map(|&i| widths[i] + gap).sum::<f32>() - gap;
            let mut x = ((w - total) / 2.0).floor();
            for i in row {
                let (tool, text) = &labels[i];
                let rect = [x, y, widths[i], TOOL_HEIGHT];
                let usable = self.usable(*tool);
                let on = match tool {
                    Tool::Toggle => matches!(self.selected, Some(Target::Custom(i))
                        if self.controls.settings.custom.get(i).is_some_and(|b| b.toggle)),
                    Tool::Hand => self.controls.settings.left_handed,
                    _ => false,
                };
                let back = if on { 0xe0ff_ffff } else { 0xd020_2428 };
                batch.fill(atlas, rect[0], rect[1], rect[2], rect[3], back);
                draw::frame(batch, atlas, rect, 1.0, 0x80ff_ffff);
                let color = match (usable, on) {
                    (false, _) => 0xff70_7070,
                    (true, true) => 0xff10_1418,
                    (true, false) => 0xffff_ffff,
                };
                draw::label(
                    batch,
                    atlas,
                    text,
                    [x + rect[2] / 2.0, y + TOOL_HEIGHT / 2.0],
                    rect[2],
                    color,
                );
                self.tools.push((*tool, rect));
                x += widths[i] + gap;
            }
            y += TOOL_HEIGHT + gap;
        }
        let note = crate::lang::get(self.scheme_key());
        let tw = HudBatch::text_width(atlas, &note);
        batch.text(
            atlas,
            &note,
            ((w - tw) / 2.0).floor(),
            y + 2.0,
            0xffe0_e0e0,
            true,
        );
    }

    fn draw_keys(&mut self, batch: &mut HudBatch, atlas: &HudAtlas) {
        let (w, h) = (batch.width, batch.height);
        let columns = ((w - 24.0) / (KEY_CELL[0] + 2.0)).floor().clamp(4.0, 12.0) as usize;
        let rows = KEYS.len().div_ceil(columns);
        let panel_w = columns as f32 * (KEY_CELL[0] + 2.0) + 10.0;
        let panel_h = rows as f32 * (KEY_CELL[1] + 2.0) + 24.0;
        let (px, py) = (
            ((w - panel_w) / 2.0).floor(),
            ((h - panel_h) / 2.0).floor().max(2.0),
        );
        batch.fill(atlas, px, py, panel_w, panel_h, 0xf010_1418);
        draw::frame(batch, atlas, [px, py, panel_w, panel_h], 1.0, 0xc0ff_ffff);
        let title = crate::lang::get("horizon.touch.editor.pick_key");
        let tw = HudBatch::text_width(atlas, &title);
        batch.text(
            atlas,
            &title,
            ((w - tw) / 2.0).floor(),
            py + 5.0,
            0xffff_ffff,
            true,
        );
        let current = match self.selected {
            Some(Target::Custom(i)) => self.controls.settings.custom.get(i).map(|b| b.key.clone()),
            _ => None,
        };
        self.keys.clear();
        for (n, key) in KEYS.iter().enumerate() {
            let (col, row) = (n % columns, n / columns);
            let rect = [
                px + 5.0 + col as f32 * (KEY_CELL[0] + 2.0),
                py + 18.0 + row as f32 * (KEY_CELL[1] + 2.0),
                KEY_CELL[0],
                KEY_CELL[1],
            ];
            let on = current.as_deref() == Some(*key);
            batch.fill(
                atlas,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                if on { 0xe0ff_ffff } else { 0xff30_3438 },
            );
            let text = horizon_client_core::key_display::display_name(key).plain();
            let color = if on { 0xff10_1418 } else { 0xffff_ffff };
            draw::label(
                batch,
                atlas,
                &text,
                [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0],
                rect[2] - 2.0,
                color,
            );
            self.keys.push((key, rect));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor as drawn on a 400 by 200 GUI, without drawing.
    fn editor() -> Editor {
        let mut e = Editor::new(Settings::default());
        e.gui = [400.0, 200.0];
        e.controls.viewport(400, 200);
        e
    }

    #[test]
    fn dragging_a_control_moves_it_and_a_new_button_takes_a_key() {
        let mut e = editor();
        let jump = e.controls.control_rect(Control::Jump);
        let at = [
            (jump[0] + jump[2] / 2.0) * 400.0,
            (jump[1] + jump[3] / 2.0) * 200.0,
        ];
        e.touch(1, at, true, false);
        assert_eq!(e.selected, Some(Target::Control(Control::Jump)));
        e.touch(1, [200.0, 100.0], false, false);
        e.touch(1, [200.0, 100.0], false, true);
        let moved = e.settings().layouts[Mode::Crosshair.index()][Control::Jump as usize];
        assert!((moved.center[0] - 0.5).abs() < 0.001 && (moved.center[1] - 0.5).abs() < 0.001);
        // Left-handed layouts store the mirror of where the control shows.
        e.apply(Tool::Hand);
        e.touch(2, [200.0, 100.0], true, false);
        assert_eq!(e.selected, Some(Target::Control(Control::Jump)));
        e.touch(2, [160.0, 100.0], false, false);
        let moved = e.settings().layouts[Mode::Crosshair.index()][Control::Jump as usize];
        assert!((moved.center[0] - 0.6).abs() < 0.001);

        e.apply(Tool::Add);
        assert!(e.picking_key);
        e.keys = vec![("key.keyboard.r", [0.0, 0.0, 10.0, 10.0])];
        e.touch(3, [5.0, 5.0], true, false);
        assert!(!e.picking_key);
        assert_eq!(e.settings().custom[0].key, "key.keyboard.r");
        e.apply(Tool::Delete);
        assert!(e.settings().custom.is_empty() && e.selected.is_none());
    }
}
