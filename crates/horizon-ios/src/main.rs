//! Horizon on iOS: the app's executable. iOS starts it with no command line, so it builds the
//! game's: the resources inside the app bundle, the game folder in the app's Documents (visible
//! in the Files app), the log and crash reports beside it.
#![deny(unsafe_code)]

use std::path::PathBuf;

#[cfg(target_os = "ios")]
mod display;
mod languages;
#[cfg(target_os = "ios")]
mod pager;

#[global_allocator]
static ALLOCATOR: horizon_threads::Allocator = horizon_threads::ALLOCATOR;

/// The mod API level this game serves (see horizon-client's main.rs).
static MOD_API_LEVEL: [u8; 27] = horizon_mods::api_level::MARK;

fn main() {
    std::hint::black_box(&MOD_API_LEVEL);
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let documents = home.join("Documents");
    let game_dir = documents.join("Horizon");
    let horizon_home = game_dir.join(".horizon");
    for dir in [&game_dir, &horizon_home] {
        let _ = std::fs::create_dir_all(dir);
    }
    let resources = bundle.join("GameData");
    // The app ships its resources packed (`xtask ios-pack`): read from the one file from the start.
    horizon_core::prepared_files::open(&resources);
    // Of vanilla's languages it ships English; the others are fetched when the player picks one.
    let languages = languages::Languages::new(&resources, home.join("Library/Caches/horizon-lang"));
    horizon_client::lang::install_remote(Box::leak(Box::new(languages)));
    let mut args = vec![
        "--resources".to_owned(),
        resources.display().to_string(),
        "--game-dir".to_owned(),
        game_dir.display().to_string(),
        "--home".to_owned(),
        horizon_home.display().to_string(),
    ];
    #[cfg(target_os = "ios")]
    horizon_client_input::window::install_display_shape(display::shape);
    #[cfg(target_os = "ios")]
    horizon_client_input::window::install_pager(pager::HOST);
    // Extra arguments for development: `simctl launch <device> <id> --world Test`. `--deck`
    // lays out iPhone Duo's upright deck at mid-height where the display has no such fold
    // (the Simulator never turns the inner display upright).
    for arg in std::env::args().skip(1) {
        if arg == "--deck" {
            horizon_client_input::window::FORCE_DIVISION
                .store(true, std::sync::atomic::Ordering::Relaxed);
        } else {
            args.push(arg);
        }
    }
    horizon_client::entry::main_with_args(args);
}
