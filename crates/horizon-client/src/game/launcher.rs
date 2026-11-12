//! The launcher the iOS app opens on (`--launcher`; its screen is `launcher_ui`): the startup
//! menu shows it over the title until Play, while the resources load behind it, and the game
//! takes over only after Play (`Loading::ready_for_handoff`). This side keeps the screen's view
//! of the game current and does what its rows ask: Play, the Microsoft account
//! (`horizon_client::msa`), the controls, and files to import.
use super::loading_screen::Loading;
use super::*;
use horizon_client::launcher_import::{self, Kind};
use horizon_client::launcher_ui::{self, Account, Action as LauncherAction, ControlScheme, SignIn};
use horizon_client::msa;
use horizon_client::options_ui::Page;
use horizon_client::touch::editor::Editor;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// What the launcher keeps between starts: `horizon-launcher.json` in the game folder.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    #[serde(with = "scheme_name")]
    scheme: ControlScheme,
}

mod scheme_name {
    use horizon_client::launcher_ui::ControlScheme;
    pub fn serialize<S: serde::Serializer>(v: &ControlScheme, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match v {
            ControlScheme::Gestures => "gestures",
            ControlScheme::Buttons => "buttons",
        })
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ControlScheme, D::Error> {
        let name: String = serde::Deserialize::deserialize(d)?;
        Ok(match name.as_str() {
            "gestures" => ControlScheme::Gestures,
            _ => ControlScheme::Buttons,
        })
    }
}

const PREFS: &str = "horizon-launcher.json";

/// The player chose the on-screen buttons in the launcher (Controls).
pub(super) fn buttons_chosen() -> bool {
    Prefs::load().scheme == ControlScheme::Buttons
}

impl Prefs {
    fn load() -> Self {
        std::fs::read(Game::game_dir().join(PREFS))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(text) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(Game::game_dir().join(PREFS), text);
        }
    }
}

/// The launcher's side in the startup menu.
pub(super) struct LauncherHost {
    /// Play was pressed and the game may take over.
    pub(super) launched: bool,
    /// Play was pressed before the title's resources arrived: it goes ahead once they do.
    play_waiting: bool,
    opened: bool,
    prefs: Prefs,
    /// The remembered account's session being renewed at start.
    restoring: Option<
        mpsc::Receiver<std::result::Result<Option<horizon_client::auth::AuthSession>, String>>,
    >,
    sign_in: Option<msa::SignIn>,
    session: Option<horizon_client::auth::AuthSession>,
    notice: Option<String>,
    /// Files being brought in, on a worker: what became of each.
    importing: Option<mpsc::Receiver<Vec<Imported>>>,
    /// The Horizon mods installed and how many of them are on, counted again after an import.
    mods: (usize, usize),
    /// The on-screen controls' layout editor, over the title instead of the launcher.
    editor: Option<Box<Editor>>,
    /// Where the mouse is, in GUI pixels, for the editor.
    pointer: [f32; 2],
    /// Frames drawn with the launcher up, for `HORIZON_LAUNCHER_SHOT`.
    frames: u32,
}

/// What became of one file brought in.
enum Imported {
    /// Added: its name, its kind and the mods to turn on (a mod's own id, a modpack's mods).
    Added(String, Kind, Vec<String>),
    Failed(String, String),
    Unknown(String),
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Brings `path` into the game folder `home` (the worker's half of Import).
fn import_one(path: &Path, home: &Path) -> Imported {
    let name = file_name(path);
    let Some(kind) = launcher_import::classify(path) else {
        return Imported::Unknown(name);
    };
    let result = match kind {
        Kind::Modpack => install_modpack(path, home),
        Kind::Layout => std::fs::create_dir_all(home)
            .and_then(|()| std::fs::copy(path, home.join(super::touch::LAYOUT_FILE)))
            .map(|_| Vec::new())
            .map_err(|e| e.to_string()),
        _ => launcher_import::import(path, kind, home)
            .map_err(|e| e.to_string())
            .map(|landed| match kind {
                Kind::Mod => horizon_mods::Package::read(&landed)
                    .map(|p| vec![p.manifest.id])
                    .unwrap_or_default(),
                _ => Vec::new(),
            }),
    };
    // The picker's copy is the app's own: done with it.
    let _ = std::fs::remove_file(path);
    match result {
        Ok(mods) => Imported::Added(name, kind, mods),
        Err(why) => Imported::Failed(name, why),
    }
}

/// A `.hpack`: its overrides into the game folder, then its mods from Horizon's registry, as the
/// desktop launcher installs a modpack's. The mods it turns on.
fn install_modpack(path: &Path, home: &Path) -> std::result::Result<Vec<String>, String> {
    use horizon_client::mod_registry;
    let pack = horizon_mods::modpack::Modpack::read(path)?;
    horizon_mods::modpack::Modpack::extract_overrides(path, home)?;
    if pack.manifest.mods.is_empty() {
        return Ok(Vec::new());
    }
    let index = mod_registry::load_index(home, &mod_registry::location())?;
    let mut on = Vec::new();
    for m in &pack.manifest.mods {
        mod_registry::install(&index, home, &m.id, &mod_registry::download)?;
        if m.on {
            on.push(m.id.clone());
        }
    }
    Ok(on)
}

/// The Horizon mods in the game folder, and how many are on.
fn count_mods(mods: &horizon_client::mod_list::ModList) -> (usize, usize) {
    let found = horizon_mods::find(&Game::game_dir().join(horizon_client::mod_list::DIR));
    let on = found
        .iter()
        .filter_map(|path| horizon_mods::Package::read(path).ok())
        .filter(|p| mods.enabled.contains(&p.manifest.id))
        .count();
    (on, found.len())
}

impl LauncherHost {
    pub(super) fn new() -> Self {
        // The remembered account, renewed on a thread while the launcher shows.
        let (send, restoring) = mpsc::channel();
        let remembered = msa::remembered().is_some();
        if remembered {
            let _ = std::thread::Builder::new()
                .name("Account".into())
                .spawn(move || {
                    let _ = send.send(msa::restore());
                });
        }
        Self {
            launched: false,
            play_waiting: false,
            opened: false,
            prefs: Prefs::load(),
            restoring: remembered.then_some(restoring),
            sign_in: None,
            session: None,
            notice: None,
            importing: None,
            mods: (0, 0),
            editor: None,
            pointer: [0.0; 2],
            frames: 0,
        }
    }
}

/// What hosts the launcher: the startup screen before the game starts, and the game's title
/// after Quit Game, which goes back to it instead of ending the app.
pub(super) trait Seat {
    /// Where the launcher's state is kept while it is up.
    fn launcher_slot(&mut self) -> &mut Option<LauncherHost>;
    /// The settings screens the launcher is a page of.
    fn launcher_ui(&mut self) -> &mut horizon_client::options_ui::OptionsUi;
    /// Shows the launcher's page.
    fn show_launcher(&mut self);
    /// The launcher's page is showing.
    fn launcher_shown(&self) -> bool;
    /// Closes it, back to the title.
    fn hide_launcher(&mut self);
    fn launcher_mods(&mut self) -> &mut horizon_client::mod_list::ModList;
    /// The title's resources are ready: Play can go ahead.
    fn launcher_ready(&self) -> bool;
    fn launcher_atlas(&self) -> Option<&horizon_client::hud::HudAtlas>;
    /// The surface's size, in pixels.
    fn launcher_surface(&self) -> [u32; 2];
    fn launcher_player_name(&self) -> String;
    fn set_launcher_player_name(&mut self, name: String);
    /// Once Play is done the launcher is gone (the game's), or stays to say so (the startup
    /// screen's, until it hands over).
    fn drops_launcher_after_play(&self) -> bool;
    /// Development hooks, once a frame with the launcher up (`HORIZON_LAUNCHER_SHOT`).
    fn launcher_tested(&mut self, _host: &mut LauncherHost) {}

    /// Once a frame while the launcher is up: its view of the game, and what it asked for.
    fn launcher_frame(&mut self) {
        let Some(mut host) = self.launcher_slot().take() else {
            return;
        };
        if !host.launched {
            self.launcher_step(&mut host);
        }
        if !(host.launched && self.drops_launcher_after_play()) {
            *self.launcher_slot() = Some(host);
        }
    }

    fn launcher_step(&mut self, host: &mut LauncherHost) {
        if !host.opened {
            host.opened = true;
            host.mods = count_mods(self.launcher_mods());
            self.show_launcher();
        }
        if let Some(restoring) = &host.restoring
            && let Ok(result) = restoring.try_recv()
        {
            host.restoring = None;
            match result {
                Ok(session) => host.session = session,
                Err(e) => host.notice = Some(e),
            }
        }
        if let Some(sign_in) = &mut host.sign_in
            && let msa::Progress::Done(_) = sign_in.poll()
        {
            host.session = sign_in.take_session();
            host.sign_in = None;
        }
        self.launcher_imports(host);
        for action in self.launcher_ui().take_launcher_actions() {
            self.launcher_action(host, action);
        }
        if host.editor.as_ref().is_some_and(|e| e.done) {
            if let Some(editor) = host.editor.take()
                && let Err(e) = editor.settings().save(&super::touch::layout_path())
            {
                host.notice = Some(e.to_string());
            }
            self.show_launcher();
            self.launcher_ui().show_launcher_page("controls");
        }
        // Escape on the launcher's own page (its last) is Play too: there is nothing behind it.
        if !self.launcher_shown() && !host.launched && host.editor.is_none() {
            host.play_waiting = true;
        }
        if host.play_waiting && self.launcher_ready() {
            self.launcher_play(host);
            return;
        }
        let state = self.launcher_view(host);
        *self.launcher_ui().launcher_state() = state;
        if self.launcher_ready() {
            host.frames += 1;
            self.launcher_tested(host);
        }
    }

    fn launcher_view(&self, host: &LauncherHost) -> launcher_ui::LauncherState {
        let offline_name = self.launcher_player_name();
        let sign_in = host.sign_in.as_ref().map(|job| {
            let mut view = SignIn::default();
            match &job.last {
                msa::Progress::Requesting => {
                    view.status = horizon_client::lang::get("horizon.launcher.sign_in.requesting")
                }
                msa::Progress::Code { code, url } => {
                    view.code = Some(code.clone());
                    view.url = url.clone();
                    view.status = horizon_client::lang::get("horizon.launcher.sign_in.waiting");
                }
                msa::Progress::Finishing | msa::Progress::Done(_) => {
                    view.status = horizon_client::lang::get("horizon.launcher.sign_in.finishing")
                }
                msa::Progress::Failed(why) => {
                    view.failed = true;
                    view.status = horizon_client::lang::get("horizon.launcher.sign_in.failed")
                        .replace("%s", why);
                }
            }
            view
        });
        launcher_ui::LauncherState {
            account: match &host.session {
                Some(session) => Account::Microsoft(session.name.clone()),
                None => Account::Offline,
            },
            offline_name,
            sign_in,
            sign_in_available: msa::client_id().is_some(),
            profiles: vec![horizon_client::lang::get(
                "horizon.launcher.profile.default",
            )],
            profile: 0,
            scheme: host.prefs.scheme,
            mods: host.mods,
            version: horizon_client::crash::version(),
            loading: if host.importing.is_some() {
                Some(horizon_client::lang::get("horizon.launcher.import.working"))
            } else {
                (host.play_waiting && !self.launcher_ready())
                    .then(|| horizon_client::lang::get("horizon.launcher.loading"))
            },
            notice: host.notice.clone(),
        }
    }

    fn launcher_action(&mut self, host: &mut LauncherHost, action: LauncherAction) {
        match action {
            LauncherAction::Play => host.play_waiting = true,
            LauncherAction::Profile(_) => {}
            LauncherAction::SignIn => {
                host.notice = None;
                host.sign_in = msa::SignIn::start();
            }
            LauncherAction::OpenSignInPage => {
                if let Some(msa::Progress::Code { code, url }) =
                    host.sign_in.as_ref().map(|job| &job.last)
                {
                    let page = format!("{url}?otc={code}");
                    match launcher_ui::platform() {
                        Some(platform) => {
                            platform.copy_text(code);
                            platform.open_url(&page);
                        }
                        None => super::packs::open_uri(&page),
                    }
                }
            }
            LauncherAction::CancelSignIn => host.sign_in = None,
            LauncherAction::SignOut => {
                msa::sign_out();
                host.session = None;
            }
            LauncherAction::ControlScheme(scheme) => {
                host.prefs.scheme = scheme;
                host.prefs.save();
            }
            LauncherAction::EditLayout => {
                let settings = horizon_client::touch::Settings::load(&super::touch::layout_path());
                host.editor = Some(Box::new(Editor::new(settings)));
                self.hide_launcher();
            }
            LauncherAction::ResetLayout => {
                let _ = std::fs::remove_file(super::touch::layout_path());
                host.notice = Some(horizon_client::lang::get(
                    "horizon.launcher.controls.reset_done",
                ));
            }
            LauncherAction::ExportLayout => {
                let path = super::touch::layout_path();
                let saved = path.is_file()
                    || horizon_client::touch::Settings::default()
                        .save(&path)
                        .is_ok();
                if let (true, Some(platform)) = (saved, launcher_ui::platform()) {
                    platform.share_file(&path);
                }
            }
            LauncherAction::Import => {
                if let Some(platform) = launcher_ui::platform() {
                    platform.pick_files();
                }
            }
            LauncherAction::RemoveProfile(_) => {}
        }
    }

    /// The layout editor, drawn over the title while it is open.
    fn launcher_editor_draw(&mut self, batch: &mut horizon_client::hud::HudBatch) {
        let Some(mut host) = self.launcher_slot().take() else {
            return;
        };
        if let (Some(editor), Some(atlas)) = (host.editor.as_mut(), self.launcher_atlas()) {
            editor.draw(batch, atlas);
        }
        *self.launcher_slot() = Some(host);
    }

    /// A finger (or the mouse, as finger 0) for the layout editor while it is open: whether it
    /// took the event.
    fn launcher_editor_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
        let scale = horizon_client::hud::HudBatch::new(self.launcher_surface()).scale;
        let Some(host) = self.launcher_slot().as_mut() else {
            return false;
        };
        let Some(editor) = host.editor.as_mut() else {
            return false;
        };
        let gui = |x: f64, y: f64| [x as f32 / scale, y as f32 / scale];
        match event {
            // A connected mouse's press comes as its own events too.
            WindowEvent::Touch(touch) if super::touch::is_pointer(touch) => {}
            WindowEvent::Touch(touch) => {
                let ended = matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled);
                let started = touch.phase == TouchPhase::Started;
                let at = gui(touch.location.x, touch.location.y);
                // Finger 0 is the mouse's.
                editor.touch(touch.id + 1, at, started, ended);
            }
            WindowEvent::CursorMoved { position, .. } => {
                host.pointer = gui(position.x, position.y);
                editor.touch(0, host.pointer, false, false);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let down = *state == ElementState::Pressed;
                editor.touch(0, host.pointer, down, !down);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key
                        == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =>
            {
                editor.apply(horizon_client::touch::editor::Tool::Done);
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::MouseWheel { .. } => {}
            _ => return false,
        }
        true
    }

    /// Files the player picked: brought in on a worker, then told in the launcher's notice, the
    /// mods among them turned on.
    fn launcher_imports(&mut self, host: &mut LauncherHost) {
        if let Some(importing) = &host.importing
            && let Ok(results) = importing.try_recv()
        {
            host.importing = None;
            let mut lines = Vec::new();
            let mut turned_on = false;
            for result in results {
                lines.push(match result {
                    Imported::Added(name, kind, mods) => {
                        for id in mods {
                            self.launcher_mods().turn_on(&id);
                            turned_on = true;
                        }
                        horizon_client::lang::get("horizon.launcher.import.done")
                            .replacen("%s", &name, 1)
                            .replacen("%s", &horizon_client::lang::get(kind.key()), 1)
                    }
                    Imported::Failed(name, why) => {
                        horizon_client::lang::get("horizon.launcher.import.failed")
                            .replacen("%s", &name, 1)
                            .replacen("%s", &why, 1)
                    }
                    Imported::Unknown(name) => {
                        horizon_client::lang::get("horizon.launcher.import.unknown")
                            .replace("%s", &name)
                    }
                });
            }
            if turned_on {
                self.launcher_mods().save(&Game::game_dir());
            }
            host.mods = count_mods(self.launcher_mods());
            host.notice = Some(lines.join("\n"));
        }
        if host.importing.is_some() {
            return;
        }
        let picked: Vec<PathBuf> = launcher_ui::platform()
            .map(|platform| platform.take_picked())
            .unwrap_or_default();
        if picked.is_empty() {
            return;
        }
        let (send, receive) = mpsc::channel();
        let home = Game::game_dir();
        let spawned = std::thread::Builder::new()
            .name("Import".into())
            .spawn(move || {
                let results = picked.iter().map(|path| import_one(path, &home)).collect();
                let _ = send.send(results);
            });
        if spawned.is_ok() {
            host.importing = Some(receive);
        }
    }

    /// Play: the launcher closes on the title, and the game may take over once it is drawn,
    /// signed in as the account, if any.
    fn launcher_play(&mut self, host: &mut LauncherHost) {
        host.launched = true;
        host.play_waiting = false;
        host.sign_in = None;
        self.hide_launcher();
        if let Some(session) = host.session.take() {
            self.set_launcher_player_name(session.name.clone());
            horizon_client::auth::set_current(session);
            horizon_client::auth::prepare_chat_keys(true);
        }
    }
}

impl Seat for Loading {
    fn launcher_slot(&mut self) -> &mut Option<LauncherHost> {
        &mut self.menu.launcher
    }
    fn launcher_ui(&mut self) -> &mut horizon_client::options_ui::OptionsUi {
        &mut self.menu.options
    }
    fn show_launcher(&mut self) {
        self.open_menu_options(Page::Launcher);
    }
    fn launcher_shown(&self) -> bool {
        self.menu.options_open
    }
    fn hide_launcher(&mut self) {
        self.menu.options_open = false;
        self.menu.dragging = false;
    }
    fn launcher_mods(&mut self) -> &mut horizon_client::mod_list::ModList {
        &mut self.settings.mods
    }
    fn launcher_ready(&self) -> bool {
        self.gui.is_some()
    }
    fn launcher_atlas(&self) -> Option<&horizon_client::hud::HudAtlas> {
        self.gui.as_ref().map(|gui| &gui.atlas)
    }
    fn launcher_surface(&self) -> [u32; 2] {
        self.menu_size()
    }
    fn launcher_player_name(&self) -> String {
        self.options
            .as_ref()
            .map_or_else(String::new, Options::player_name)
    }
    fn set_launcher_player_name(&mut self, name: String) {
        if let Some(options) = &mut self.options {
            options.player_name = Some(name);
        }
    }
    fn drops_launcher_after_play(&self) -> bool {
        false
    }
    fn launcher_tested(&mut self, host: &mut LauncherHost) {
        // `HORIZON_LAUNCHER_SHOT=<png>,editor`: the layout editor.
        if host.frames == 60
            && std::env::var("HORIZON_LAUNCHER_SHOT").is_ok_and(|v| v.ends_with(",editor"))
        {
            self.launcher_action(host, LauncherAction::EditLayout);
        }
        self.launcher_shot(host.frames);
    }
}

impl Loading {
    /// `HORIZON_LAUNCHER_SHOT=<png>[,<page>]`, for development: once the launcher has been up
    /// for a moment, show that page, save a picture of it and quit.
    fn launcher_shot(&mut self, frames: u32) {
        let Some(spec) = std::env::var_os("HORIZON_LAUNCHER_SHOT") else {
            return;
        };
        let spec = spec.to_string_lossy().into_owned();
        let (path, page) = spec.split_once(',').unwrap_or((spec.as_str(), "play"));
        // `played`: press Play (the log says when the game takes over).
        if page == "played" {
            if frames == 60 {
                self.menu.options.launcher_press_play();
            }
            return;
        }
        match frames {
            60 => self.menu.options.show_launcher_page(page),
            90 => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.screenshot = Some(horizon_client::gfx::ScreenshotRequest {
                        path: path.into(),
                        size: None,
                        scaled: false,
                        background: false,
                    });
                }
            }
            120 => std::process::exit(0),
            _ => {}
        }
    }
}

impl Seat for Game<'_> {
    fn launcher_slot(&mut self) -> &mut Option<LauncherHost> {
        &mut self.launcher
    }
    fn launcher_ui(&mut self) -> &mut horizon_client::options_ui::OptionsUi {
        &mut self.ui.options
    }
    fn show_launcher(&mut self) {
        self.open_options_at(true, Page::Launcher);
    }
    fn launcher_shown(&self) -> bool {
        self.screen == Screen::Options
    }
    fn hide_launcher(&mut self) {
        self.settings.save(&Self::options_path());
        self.screen = Screen::Playing;
        self.dragging = false;
        // The scheme or the layout may have changed.
        self.touch_reload_scheme();
    }
    fn launcher_mods(&mut self) -> &mut horizon_client::mod_list::ModList {
        &mut self.settings.mods
    }
    fn launcher_ready(&self) -> bool {
        true
    }
    fn launcher_atlas(&self) -> Option<&horizon_client::hud::HudAtlas> {
        Some(&self.gui.atlas)
    }
    fn launcher_surface(&self) -> [u32; 2] {
        self.renderer
            .as_ref()
            .map_or([1280, 720], |r| [r.config.width, r.config.height])
    }
    fn launcher_player_name(&self) -> String {
        self.options.player_name()
    }
    fn set_launcher_player_name(&mut self, name: String) {
        self.options.player_name = Some(name);
    }
    fn drops_launcher_after_play(&self) -> bool {
        true
    }
}

impl Game<'_> {
    /// Quit Game on the title of a game the launcher started (the iOS app): back to the
    /// launcher, as an app on a phone doesn't end itself. Whether it went there.
    pub(super) fn quit_to_launcher(&mut self) -> bool {
        if !self.options.launcher || self.title.is_none() {
            return false;
        }
        if self.launcher.is_none() {
            self.launcher = Some(LauncherHost::new());
        }
        true
    }
}
