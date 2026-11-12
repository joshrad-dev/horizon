//! The launcher the iOS app opens on (`--launcher`; its screen is `launcher_ui`): the startup
//! menu shows it over the title until Play, while the resources load behind it, and the game
//! takes over only after Play (`Loading::ready_for_handoff`). This side keeps the screen's view
//! of the game current and does what its rows ask: Play, the Microsoft account
//! (`horizon_client::msa`), the controls, and files to import.
use super::loading_screen::Loading;
use super::*;
use horizon_client::launcher_ui::{self, Account, Action as LauncherAction, ControlScheme, SignIn};
use horizon_client::msa;
use horizon_client::options_ui::Page;
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
            "buttons" => ControlScheme::Buttons,
            _ => ControlScheme::Gestures,
        })
    }
}

const PREFS: &str = "horizon-launcher.json";

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
    /// Frames drawn with the launcher up, for `HORIZON_LAUNCHER_SHOT`.
    frames: u32,
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
            frames: 0,
        }
    }
}

impl Loading {
    /// Once a frame while the launcher is up: its view of the game, and what it asked for.
    pub(super) fn launcher_frame(&mut self) {
        let Some(mut host) = self.menu.launcher.take() else {
            return;
        };
        if !host.launched {
            self.launcher_step(&mut host);
        }
        self.menu.launcher = Some(host);
    }

    fn launcher_step(&mut self, host: &mut LauncherHost) {
        if !host.opened {
            host.opened = true;
            self.open_menu_options(Page::Launcher);
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
        for action in self.menu.options.take_launcher_actions() {
            self.launcher_action(host, action);
        }
        // Escape on the launcher's own page (its last) is Play too: there is nothing behind it.
        if !self.menu.options_open && !host.launched {
            host.play_waiting = true;
        }
        if host.play_waiting && self.gui.is_some() {
            self.launcher_play(host);
            return;
        }
        let state = self.launcher_view(host);
        *self.menu.options.launcher_state() = state;
        if self.gui.is_some() {
            host.frames += 1;
            self.launcher_shot(host.frames);
        }
    }

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

    fn launcher_view(&self, host: &LauncherHost) -> launcher_ui::LauncherState {
        let offline_name = self
            .options
            .as_ref()
            .map_or_else(String::new, Options::player_name);
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
        let mods = &self.settings.mods;
        let on = mods
            .installed
            .iter()
            .filter(|m| mods.enabled.contains(&m.id))
            .count();
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
            mods: (on, mods.installed.len()),
            version: horizon_client::crash::version(),
            loading: (host.play_waiting && self.gui.is_none())
                .then(|| horizon_client::lang::get("horizon.launcher.loading")),
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
            LauncherAction::EditLayout
            | LauncherAction::ResetLayout
            | LauncherAction::ExportLayout => {}
            LauncherAction::Import => {
                if let Some(platform) = launcher_ui::platform() {
                    platform.pick_files();
                }
            }
            LauncherAction::RemoveProfile(_) => {}
        }
    }

    /// Play: the launcher closes on the title, and the game may take over once it is drawn,
    /// signed in as the account, if any.
    fn launcher_play(&mut self, host: &mut LauncherHost) {
        host.launched = true;
        host.play_waiting = false;
        host.sign_in = None;
        self.menu.options_open = false;
        self.menu.dragging = false;
        if let Some(session) = host.session.take() {
            if let Some(options) = &mut self.options {
                options.player_name = Some(session.name.clone());
            }
            horizon_client::auth::set_current(session);
            horizon_client::auth::prepare_chat_keys(true);
        }
    }
}
