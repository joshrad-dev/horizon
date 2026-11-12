//! The launcher (the iOS app's, `--launcher`): the screen the app opens on, before the title, on
//! the settings shell (`settings_shell`). Its pages: Play, the Microsoft account the game joins
//! servers as, the touch controls, the library (mods, shader packs, resource packs, modpacks) and
//! what this build is. What its rows show comes from the game ([`LauncherState`], which it keeps
//! current); what a press does goes back to it as an [`Action`].
use crate::{
    lang,
    options_ui::{Event, Page},
    settings::Settings,
    settings_shell::{self as shell, Group, Output, Row, Screen, Sheet, Value},
};

const TITLE: &str = "horizon.launcher";

/// What the launcher needs of the system it runs on (the iOS app's, horizon-ios), which this
/// crate can't do itself: each does nothing where the platform installed none.
pub trait Platform: Send + Sync {
    /// Open a web page in the browser.
    fn open_url(&self, url: &str);
    /// Put text on the system's clipboard.
    fn copy_text(&self, text: &str);
    /// Ask the player for files to bring in; they arrive later through [`Platform::take_picked`],
    /// as copies the game may move.
    fn pick_files(&self);
    /// The files picked since the last call.
    fn take_picked(&self) -> Vec<std::path::PathBuf>;
    /// Offer a file to the system's share sheet (Save to Files, AirDrop...).
    fn share_file(&self, path: &std::path::Path);
}

static PLATFORM: std::sync::OnceLock<&'static dyn Platform> = std::sync::OnceLock::new();

/// Installs the platform's half of the launcher. Only the first call counts.
pub fn install_platform(platform: &'static dyn Platform) {
    let _ = PLATFORM.set(platform);
}

/// The installed platform, if any.
pub fn platform() -> Option<&'static dyn Platform> {
    PLATFORM.get().copied()
}

/// What the launcher asks the game to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start: the title, then the game, with the chosen profile and account.
    Play,
    /// Play this profile (`LauncherState::profiles`).
    Profile(usize),
    /// Sign in with a Microsoft account (the device-code flow).
    SignIn,
    /// Open the sign-in page with the code the flow gave.
    OpenSignInPage,
    /// Stop the sign-in under way.
    CancelSignIn,
    /// Forget the account: play offline.
    SignOut,
    /// The touch controls: gestures, or the on-screen buttons of a layout.
    ControlScheme(ControlScheme),
    /// Edit the on-screen buttons' layout.
    EditLayout,
    /// Put the layout back to the built-in one.
    ResetLayout,
    /// Pick a file to bring in: a mod, a modpack, a resource or shader pack, a layout.
    Import,
    /// Share the layout's file.
    ExportLayout,
    /// Remove modpack profile `.0` (`LauncherState::profiles`, never the default).
    RemoveProfile(usize),
}

/// The touch controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlScheme {
    /// The Pocket Edition's gestures, no buttons (game/touch.rs).
    #[default]
    Gestures,
    /// On-screen buttons, placed and bound in the layout editor.
    Buttons,
}

/// What the account row shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Account {
    /// No Microsoft account: the game plays offline as this name.
    #[default]
    Offline,
    /// Signed in: the profile's name.
    Microsoft(String),
}

/// A sign-in under way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignIn {
    /// The code to type on the sign-in page, once Microsoft gave it.
    pub code: Option<String>,
    /// The page to type it on.
    pub url: String,
    /// What it is doing now, or why it stopped (already translated).
    pub status: String,
    /// It stopped: the status says why.
    pub failed: bool,
}

/// What the game tells the launcher, before each draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherState {
    pub account: Account,
    /// The name the game plays as offline.
    pub offline_name: String,
    pub sign_in: Option<SignIn>,
    /// Sign-in is built in (an Azure app's client ID): without one, the account page says so.
    pub sign_in_available: bool,
    /// The default profile, then each installed modpack's, by name.
    pub profiles: Vec<String>,
    pub profile: usize,
    pub scheme: ControlScheme,
    /// Mods installed and on, and installed.
    pub mods: (usize, usize),
    pub version: String,
    /// Resources are still loading: Play waits for them (it says so).
    pub loading: Option<String>,
    /// The last thing that went wrong (an import, a sign-in), to show on the Play page.
    pub notice: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LauncherPage {
    Play,
    Account,
    Controls,
    Library,
    About,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LauncherRow {
    Play,
    Profile,
    AccountName,
    Notice,
    Loading,
    SignIn,
    SignInCode,
    SignInStatus,
    OpenSignInPage,
    CancelSignIn,
    SignOut,
    OfflineName,
    NoSignIn,
    Scheme,
    EditLayout,
    ResetLayout,
    ExportLayout,
    Mods,
    BrowseMods,
    ShaderPacks,
    ResourcePacks,
    Import,
    RemoveProfile,
    Options,
    Version,
    Licence,
}

#[derive(Default)]
pub(crate) struct LauncherScreen {
    pub(crate) state: LauncherState,
    /// Pressed rows' actions, for the game to take ([`crate::options_ui::OptionsUi::take_launcher_actions`]).
    pub(crate) actions: Vec<Action>,
}

fn row(id: LauncherRow, name: &str, value: Value, about: &str) -> Row<LauncherRow> {
    let mut row = Row::new(id, lang::get(name), value);
    row.about = lang::get(about);
    row
}

fn group(name: &'static str, rows: Vec<Row<LauncherRow>>) -> Group<LauncherRow> {
    Group { name, rows }
}

impl LauncherScreen {
    fn account_line(&self) -> String {
        match &self.state.account {
            Account::Microsoft(name) => {
                lang::get("horizon.launcher.account.microsoft").replace("%s", name)
            }
            Account::Offline => lang::get("horizon.launcher.account.offline")
                .replace("%s", &self.state.offline_name),
        }
    }
    fn play_page(&self) -> Sheet<LauncherPage, LauncherRow> {
        let s = &self.state;
        let mut rows = vec![row(
            LauncherRow::Play,
            "horizon.launcher.play",
            Value::Press,
            "horizon.launcher.play.about",
        )];
        if s.profiles.len() > 1 {
            let name = s.profiles.get(s.profile).cloned().unwrap_or_default();
            rows.push(row(
                LauncherRow::Profile,
                "horizon.launcher.profile",
                Value::Choice(name),
                "horizon.launcher.profile.about",
            ));
        }
        rows.push(row(
            LauncherRow::AccountName,
            "horizon.launcher.account",
            Value::Read(self.account_line()),
            "horizon.launcher.account.about",
        ));
        if let Some(loading) = &s.loading {
            rows.push(Row::new(LauncherRow::Loading, loading.clone(), Value::Note));
        }
        if let Some(notice) = &s.notice {
            rows.push(Row::new(LauncherRow::Notice, notice.clone(), Value::Note));
        }
        Sheet::new(
            LauncherPage::Play,
            lang::get("horizon.launcher.page.play"),
            lang::get("horizon.launcher.page.play.about"),
            vec![group("horizon.launcher.group.play", rows)],
        )
    }
    fn account_page(&self) -> Sheet<LauncherPage, LauncherRow> {
        let s = &self.state;
        let mut rows = vec![row(
            LauncherRow::AccountName,
            "horizon.launcher.account",
            Value::Read(self.account_line()),
            "horizon.launcher.account.about",
        )];
        match (&s.account, &s.sign_in) {
            (Account::Microsoft(_), _) => rows.push(row(
                LauncherRow::SignOut,
                "horizon.launcher.sign_out",
                Value::Press,
                "horizon.launcher.sign_out.about",
            )),
            (Account::Offline, Some(sign_in)) => {
                if let Some(code) = &sign_in.code {
                    rows.push(row(
                        LauncherRow::SignInCode,
                        "horizon.launcher.sign_in.code",
                        Value::Read(code.clone()),
                        "horizon.launcher.sign_in.code.about",
                    ));
                    let mut open = row(
                        LauncherRow::OpenSignInPage,
                        "horizon.launcher.sign_in.open",
                        Value::Press,
                        "horizon.launcher.sign_in.open.about",
                    );
                    open.about = open.about.replace("%s", &sign_in.url);
                    rows.push(open);
                }
                let mut status = Row::new(
                    LauncherRow::SignInStatus,
                    sign_in.status.clone(),
                    Value::Note,
                );
                status.live = !sign_in.failed;
                rows.push(status);
                rows.push(row(
                    LauncherRow::CancelSignIn,
                    if sign_in.failed {
                        "horizon.launcher.sign_in.back"
                    } else {
                        "horizon.launcher.sign_in.cancel"
                    },
                    Value::Press,
                    "horizon.launcher.sign_in.cancel.about",
                ));
            }
            (Account::Offline, None) if s.sign_in_available => rows.push(row(
                LauncherRow::SignIn,
                "horizon.launcher.sign_in",
                Value::Press,
                "horizon.launcher.sign_in.about",
            )),
            (Account::Offline, None) => rows.push(Row::new(
                LauncherRow::NoSignIn,
                lang::get("horizon.launcher.sign_in.unavailable"),
                Value::Note,
            )),
        }
        let offline = vec![row(
            LauncherRow::OfflineName,
            "horizon.launcher.offline_name",
            Value::Read(s.offline_name.clone()),
            "horizon.launcher.offline_name.about",
        )];
        Sheet::new(
            LauncherPage::Account,
            lang::get("horizon.launcher.page.account"),
            lang::get("horizon.launcher.page.account.about"),
            vec![
                group("horizon.launcher.group.microsoft", rows),
                group("horizon.launcher.group.offline", offline),
            ],
        )
    }
    fn controls_page(&self) -> Sheet<LauncherPage, LauncherRow> {
        let scheme = match self.state.scheme {
            ControlScheme::Gestures => "horizon.launcher.scheme.gestures",
            ControlScheme::Buttons => "horizon.launcher.scheme.buttons",
        };
        let mut scheme_row = row(
            LauncherRow::Scheme,
            "horizon.launcher.scheme",
            Value::Choice(lang::get(scheme)),
            "horizon.launcher.scheme.about",
        );
        scheme_row.chosen = Some(lang::get(&format!("{scheme}.about")));
        let buttons = self.state.scheme == ControlScheme::Buttons;
        let mut layout = vec![
            row(
                LauncherRow::EditLayout,
                "horizon.launcher.layout.edit",
                Value::Press,
                "horizon.launcher.layout.edit.about",
            ),
            row(
                LauncherRow::ExportLayout,
                "horizon.launcher.layout.export",
                Value::Press,
                "horizon.launcher.layout.export.about",
            ),
            row(
                LauncherRow::ResetLayout,
                "horizon.launcher.layout.reset",
                Value::Press,
                "horizon.launcher.layout.reset.about",
            ),
        ];
        for row in &mut layout {
            row.live = buttons;
            if !buttons {
                row.why = Some(lang::get("horizon.launcher.layout.needs_buttons"));
            }
        }
        Sheet::new(
            LauncherPage::Controls,
            lang::get("horizon.launcher.page.controls"),
            lang::get("horizon.launcher.page.controls.about"),
            vec![
                group("horizon.launcher.group.scheme", vec![scheme_row]),
                group("horizon.launcher.group.layout", layout),
            ],
        )
    }
    fn library_page(&self) -> Sheet<LauncherPage, LauncherRow> {
        let s = &self.state;
        let mut mods = row(
            LauncherRow::Mods,
            "horizon.launcher.mods",
            Value::Read(
                lang::get("horizon.launcher.mods.count")
                    .replacen("%s", &s.mods.0.to_string(), 1)
                    .replacen("%s", &s.mods.1.to_string(), 1),
            ),
            "horizon.launcher.mods.about",
        );
        mods.value = Value::Press;
        let mut profiles = Vec::new();
        if s.profiles.len() > 1 && s.profile > 0 {
            let mut remove = row(
                LauncherRow::RemoveProfile,
                "horizon.launcher.profile.remove",
                Value::Press,
                "horizon.launcher.profile.remove.about",
            );
            remove.name = remove
                .name
                .replace("%s", s.profiles.get(s.profile).map_or("", String::as_str));
            profiles.push(remove);
        }
        let mut groups = vec![
            group(
                "horizon.launcher.group.mods",
                vec![
                    mods,
                    row(
                        LauncherRow::BrowseMods,
                        "horizon.launcher.browse_mods",
                        Value::Press,
                        "horizon.launcher.browse_mods.about",
                    ),
                ],
            ),
            group(
                "horizon.launcher.group.packs",
                vec![
                    row(
                        LauncherRow::ShaderPacks,
                        "horizon.launcher.shader_packs",
                        Value::Press,
                        "horizon.launcher.shader_packs.about",
                    ),
                    row(
                        LauncherRow::ResourcePacks,
                        "horizon.launcher.resource_packs",
                        Value::Press,
                        "horizon.launcher.resource_packs.about",
                    ),
                ],
            ),
            group(
                "horizon.launcher.group.import",
                vec![row(
                    LauncherRow::Import,
                    "horizon.launcher.import",
                    Value::Press,
                    "horizon.launcher.import.about",
                )],
            ),
        ];
        if !profiles.is_empty() {
            groups.push(group("horizon.launcher.group.modpacks", profiles));
        }
        Sheet::new(
            LauncherPage::Library,
            lang::get("horizon.launcher.page.library"),
            lang::get("horizon.launcher.page.library.about"),
            groups,
        )
    }
    fn about_page(&self) -> Sheet<LauncherPage, LauncherRow> {
        Sheet::new(
            LauncherPage::About,
            lang::get("horizon.launcher.page.about"),
            lang::get("horizon.launcher.page.about.about"),
            vec![group(
                "horizon.launcher.group.about",
                vec![
                    row(
                        LauncherRow::Version,
                        "horizon.launcher.version",
                        Value::Read(self.state.version.clone()),
                        "horizon.launcher.version.about",
                    ),
                    row(
                        LauncherRow::Options,
                        "horizon.launcher.options",
                        Value::Press,
                        "horizon.launcher.options.about",
                    ),
                    Row::new(
                        LauncherRow::Licence,
                        lang::get("horizon.launcher.licence"),
                        Value::Note,
                    ),
                ],
            )],
        )
    }
    fn act(&mut self, action: Action) -> Vec<Output> {
        self.actions.push(action);
        vec![Output::Event(Event::ButtonClick)]
    }
}

impl Screen for LauncherScreen {
    type Page = LauncherPage;
    type Row = LauncherRow;
    fn title(&self) -> &'static str {
        TITLE
    }
    fn note_lines(&self) -> usize {
        3
    }
    fn done_label(&self) -> &'static str {
        "horizon.launcher.play"
    }
    fn sheets(&self, _s: &Settings) -> Vec<Sheet<LauncherPage, LauncherRow>> {
        vec![
            self.play_page(),
            self.account_page(),
            self.controls_page(),
            self.library_page(),
            self.about_page(),
        ]
    }
    fn stamp(&self, _s: &Settings) -> u64 {
        shell::stamp_of(&self.state)
    }
    fn press(&mut self, _s: &mut Settings, row: LauncherRow, back: bool) -> Vec<Output> {
        let s = &self.state;
        match row {
            LauncherRow::Play => self.act(Action::Play),
            LauncherRow::Profile => {
                let n = s.profiles.len().max(1);
                let next = if back {
                    (s.profile + n - 1) % n
                } else {
                    (s.profile + 1) % n
                };
                self.act(Action::Profile(next))
            }
            LauncherRow::SignIn => self.act(Action::SignIn),
            LauncherRow::OpenSignInPage => self.act(Action::OpenSignInPage),
            LauncherRow::CancelSignIn => self.act(Action::CancelSignIn),
            LauncherRow::SignOut => self.act(Action::SignOut),
            LauncherRow::Scheme => self.act(Action::ControlScheme(match s.scheme {
                ControlScheme::Gestures => ControlScheme::Buttons,
                ControlScheme::Buttons => ControlScheme::Gestures,
            })),
            LauncherRow::EditLayout => self.act(Action::EditLayout),
            LauncherRow::ResetLayout => self.act(Action::ResetLayout),
            LauncherRow::ExportLayout => self.act(Action::ExportLayout),
            LauncherRow::Import => self.act(Action::Import),
            LauncherRow::RemoveProfile => self.act(Action::RemoveProfile(s.profile)),
            LauncherRow::Mods => vec![Output::Event(Event::ButtonClick), Output::Push(Page::Mods)],
            LauncherRow::BrowseMods => vec![
                Output::Event(Event::ButtonClick),
                Output::Push(Page::ModBrowser),
            ],
            LauncherRow::ShaderPacks => vec![
                Output::Event(Event::ButtonClick),
                Output::Push(Page::ShaderPacks),
            ],
            LauncherRow::ResourcePacks => {
                vec![Output::Event(Event::ButtonClick), Output::Push(Page::Packs)]
            }
            LauncherRow::Options => {
                vec![Output::Event(Event::ButtonClick), Output::Push(Page::Main)]
            }
            LauncherRow::AccountName
            | LauncherRow::Notice
            | LauncherRow::Loading
            | LauncherRow::SignInCode
            | LauncherRow::SignInStatus
            | LauncherRow::OfflineName
            | LauncherRow::NoSignIn
            | LauncherRow::Version
            | LauncherRow::Licence => Vec::new(),
        }
    }
    fn link(&self, row: LauncherRow) -> Option<LauncherPage> {
        // The account line on the Play page opens the account's page.
        (row == LauncherRow::AccountName).then_some(LauncherPage::Account)
    }
}

impl crate::options_ui::OptionsUi {
    /// The launcher's view of the game, for the game to keep current.
    pub fn launcher_state(&mut self) -> &mut LauncherState {
        &mut self.launcher.screen.state
    }
    /// What the launcher's rows asked for since the last call.
    pub fn take_launcher_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.launcher.screen.actions)
    }
    /// Open the launcher, as the first and only page under it.
    pub fn open_launcher(&mut self) {
        self.open_at(false, Page::Launcher);
    }
    /// Press Play, as its button does.
    pub fn launcher_press_play(&mut self) {
        self.launcher.screen.actions.push(Action::Play);
    }
    /// Show the launcher's page named `name` (`play`, `account`, `controls`, `library`,
    /// `about`), for screenshots of it.
    pub fn show_launcher_page(&mut self, name: &str) {
        let page = match name {
            "account" => LauncherPage::Account,
            "controls" => LauncherPage::Controls,
            "library" => LauncherPage::Library,
            "about" => LauncherPage::About,
            _ => LauncherPage::Play,
        };
        self.launcher.show(page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_shell::{Hosted, Shell};

    const SIZE: (f32, f32) = (854.0, 480.0);

    /// Play, on its page and in the foot, asks the game to start; the launcher never closes
    /// itself.
    #[test]
    fn play_asks_the_game_to_start() {
        let mut s = Settings::default();
        let mut shell = Shell::<LauncherScreen>::default();
        let at = shell.point_at(&s, SIZE, LauncherRow::Play);
        let outputs = shell.click(&mut s, at, SIZE, false, false, false);
        assert!(!outputs.iter().any(|o| matches!(o, Output::Done)));
        assert_eq!(shell.screen.actions, [Action::Play]);
    }

    /// Without a client ID there is no Sign in button, only why; with one, it starts a sign-in.
    #[test]
    fn sign_in_needs_the_app_to_have_it() {
        let mut s = Settings::default();
        let mut shell = Shell::<LauncherScreen>::default();
        shell.show(LauncherPage::Account);
        let ids = |shell: &Shell<LauncherScreen>, s: &Settings| -> Vec<LauncherRow> {
            let sheets = shell.screen.sheets(s);
            let account = sheets.into_iter().find(|p| p.id == LauncherPage::Account);
            let groups = account.into_iter().flat_map(|p| p.groups);
            groups.flat_map(|g| g.rows).map(|r| r.id).collect()
        };
        assert!(ids(&shell, &s).contains(&LauncherRow::NoSignIn));
        assert!(!ids(&shell, &s).contains(&LauncherRow::SignIn));
        shell.screen.state.sign_in_available = true;
        assert!(ids(&shell, &s).contains(&LauncherRow::SignIn));
        let at = shell.point_at(&s, SIZE, LauncherRow::SignIn);
        shell.click(&mut s, at, SIZE, false, false, false);
        assert_eq!(shell.screen.actions, [Action::SignIn]);
    }
}
