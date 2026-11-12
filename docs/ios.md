# Horizon on iOS

The game runs on iPhone and iPad through wgpu's Metal backend. The iOS app is the
`horizon-ios` crate: a small executable that builds the game's command line and calls
`horizon_client::entry::main_with_args`. Everything else is the same gameplay, renderer and
server code the desktop game runs.

Status: early. The game boots, renders the title screen and worlds through Metal, single
player works (on its integrated server thread), touch controls cover the basics, and an iPad
plays with a hardware keyboard and mouse or trackpad as the desktop game does. Expect
rough edges: see [What's left](#whats-left).

## Build and run (simulator)

You need Xcode, rustup (Homebrew's `rust` has no iOS standard library), and the game's resources
prepared from your own copy of Minecraft, as for the desktop game.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# 1. The pinned Minecraft files, from Mojang (dev only, into .local/mojang; never committed),
#    and a JDK of the pinned major version for the exporters.
python3 tools/ios/fetch_minecraft.py
# 2. Prepare the resources (.local/horizon-client/26.3).
cargo build --profile fast -p horizon-prepare
./target/fast/horizon-prepare --prepare --complete --minecraft-dir .local/mojang \
    --server-jar .local/mojang/server/server-26.3.jar --java-home /path/to/jdk-25 --root "$PWD"
# 3. Build Horizon.app and install it on a booted simulator.
tools/ios/build_app.sh --install <simulator UDID>
```

`tools/ios/build_app.sh` builds `horizon-ios` with `--profile fast` (`PROFILE=release` for a
build to keep), assembles `target/ios/<target>/Horizon.app` (the executable, `Info.plist` from
`crates/horizon-ios`, and the resources under `GameData/`), and signs it ad hoc. A release build
ships a stripped executable and leaves the unstripped one beside the app
(`horizon-ios.unstripped`), for `atos -o` to read crash reports with.

For a device: `tools/ios/build_app.sh --device --sign "Apple Development: …" --profile
path/to/profile.mobileprovision [--entitlements app.entitlements]` (`BUILD_NUMBER` sets
`CFBundleVersion`), then install with Xcode's Devices window or `xcrun devicectl`.

### The app's resources, small

The prepared folder is 685 MB; the app ships about 220 MB of it (`tools/ios/stage_resources.py`,
which needs `brew install vorbis-tools`):

- **Packed**: every file but the sounds goes into one `horizon-data.bin` (version 3 of
  `crates/horizon-core/src/prepared_files.rs`), each zstd-compressed at level 19
  (`cargo run -p xtask -- ios-pack`): 150 MB of JSON and NBT in 20 MB, without the 88 MB the
  folder's 22,000 small files lose to the file system. The game reads through
  `horizon_core::resource_fs`, which is `std::fs` everywhere but under a packed folder (and a
  resource packs' mirror over one, `prepared_files::open_overlay`). Code reading the prepared
  folder or a pack's folder reads with it.
- **Languages**: English alone of vanilla's 142 lang files (85 MB). The others are fetched from
  Mojang's asset server the first time a player picks one, by the hash in `asset-index.json`, and
  kept in the app's caches (`crates/horizon-ios/src/languages.rs`). `LANGUAGES=en_us,fr_fr` ships
  more.
- **Sounds**: re-encoded to Ogg Vorbis at `oggenc -q 0` (`SOUND_QUALITY`) and at most 48 kHz:
  359 MB to 198 MB, music the most. `NO_SOUNDS=1` leaves them out.

On the device the game folder (saves, `options.txt`) is `Documents/Horizon` and the log and
crash reports are in `Documents/Horizon/.horizon`; the Files app shows them.

Extra arguments for development go after the bundle id:
`xcrun simctl launch <udid> dev.horizonlab.horizon.ios --world Test --creative`.

## Touch controls

`crates/horizon-client/src/game/touch.rs`: gestures in the manner of the classic Pocket Edition,
with no buttons, no stick drawn, no crosshair and no controller hints: nothing shows for the
controls, even while touching. Actions aim where the finger is.

Where the gestures go:

- **iPad, and iPhone Duo open flat or closed**: the thumb walks from the lower left corner of the
  game's view (its left third, lower 60%); everywhere else looks and acts.
- **iPhone Duo upright**: the deck's left side walks, its right side acts (tap, swipes up and
  down, swipes across to look, hold, double tap), and the game's whole view above the fold looks and acts.

| Gesture | On the walking area | On the look/act area |
|---|---|---|
| Drag | Walk (a floating stick); to its edge, sprint | Across: turn the view, then every way |
| Tap | | Inventory (after the double tap's window) |
| Swipe down | | Crouch until the finger lifts |
| Swipe up | | Jump until the finger lifts (swims and flies up while held) |
| Hold still | | Mine or attack under the finger (the view's centre from the deck) |
| Double tap | | Use or place at the finger (the view's centre from the deck) |
| Double tap with a finger on each | Pause | Pause |

The hotbar takes taps to select a slot.

Distances are in points, so they feel the same whatever the GUI scale: a tap moves under 10 pt,
a hold stays within the same 10 pt. Past 10 pt a finger is decided by its direction: more vertical than
across is a swipe, acting at 28 pt (up jumps, down crouches); more across is a look, which turns
the view from there on, never by the way it came, so the view never jumps.

The gestures press the virtual pad's buttons (the one `horizon-ctl pad` drives), so they do what
the controller bindings say; touch alone never marks the controller in use, so no glyphs show. The pick aims through the finger's point (`touch_aim`, used by
`aim()`). In menus a finger is the mouse (down presses, moving drags, up releases) and two
fingers scroll; **a tap outside a menu opened in play closes it** (outside a container's panel,
`hasClickedOutside`, with nothing carried; or on no widget of another screen; screens that type
are left alone). The system keyboard comes up when a text field is tapped and goes away on
Return or a tap elsewhere.

## iPhone Duo upright: the page view

`crates/horizon-client/src/game/deck.rs`, `crates/horizon-client-render/src/gfx/deck.rs` and
`crates/horizon-ios/swift/DeckPages.swift`. When the window has a horizontal fold (UIKit's
`reservedRegionsOfKind:` with the division kind, asked on every resize through
`window::display_shape`), the game takes the screen above the fold and the deck the one below: the controls' two pads at
its sides and, pressed into the metal between them with padding, a card holding the page view:

- **The pages are SwiftUI**: a paging `ScrollView` in a `UIHostingController` over the deck
  (`window::PagerHost`, implemented in `crates/horizon-ios/src/pager.rs`), with the page's
  title and its dots on Liquid-Glass-like material. It owns the paging, its physics and taps; it
  reports its offset (`onScrollGeometryChange`, iOS 18) and a queue of taps.
- **The pages' content is the game's**, drawn under the pager at its offset so it slides with it:
  the **inventory** (tap the hotbar row to select a slot), a **map** (the minimap's tiles, turning
  with the player), the **player** as they are now (the inventory's 3D preview) and their
  **status** (health, food, level, position, biome, day and time). SwiftUI can't draw Minecraft
  items or models, hence the split.
- **The reflection**: the renderer draws offscreen at the size of the top part; `DeckPresenter`
  composes each frame: the game above the fold, then brushed graphite reflecting it as metal does
  (the frame in a 256x128 mipmapped probe, mirrored across the fold, nearly sharp at the fold and
  blurring steadily with distance, smeared along the reflection more than across it, mostly grey
  with a little of the scene's hue, fading down the deck), then the recessed card (darker, shadowed
  under its top edge, a lit lip below) with the page content clipped to it.
- Folding closed hides the pager and gives the game the whole screen; opening brings it back.

`build.rs` compiles `swift/` with `swiftc` into a static library the app links. The iOS
Simulator reports the fold once the Duo is open; `--deck` (an argument of the app) lays the
deck out at mid-height where the display has no horizontal fold, for testing.

## Keyboard and mouse

`crates/horizon-ios-input` (GameController) and `crates/horizon-client/src/game/hardware_input.rs`.
A keyboard and a mouse or trackpad (the Magic Keyboard's included) play as on the desktop: their
input becomes the window events the desktop's keyboard and mouse send, so the key bindings,
screens, chat and native mods take it unchanged.

| Device | What |
|---|---|
| Keys | The desktop's keys and bindings (WASD, Space, Shift, E, the digits, T and `/`, Esc, F1 to F12 where the keyboard has them), held keys repeat |
| Text | Typed through UIKit while a text field has focus, in the keyboard's own layout; the on-screen keyboard stays away |
| Mouse in play | The pointer is locked and hidden, as the desktop grabs the mouse; its raw movement turns the view, left attacks, right uses, middle picks, the wheel changes the hotbar slot |
| Mouse over screens | The system's own pointer, with its speed and scroll settings: hover, tooltips, slot hover, clicks, drags and scrolling as on the desktop |

The touch controls hide once a key is pressed, a mouse button clicked or the pointer moved a
little (a palm brushing the trackpad doesn't count), and come back when a finger touches the
screen. The log says which is in use (`keyboard and mouse are the input in use`, `touch is the
input in use`), when a keyboard or mouse connects, and each pointer lock asked for and granted
(`pointer lock wanted`, `pointer lock requested`, `the system's pointer lock changed`, `pointer
locked`).

Touches the controls make on the virtual pad don't count as a controller: no controller guide,
glyphs or menu focus appear for them.

## Platform changes

- **Metal only**: `backend_order` takes wgpu's default on iOS as on macOS; the options hide the
  graphics API row. The iOS Simulator's GPU has no indirect execution, so the flat terrain draws
  and occlusion culling check `DownlevelFlags::INDIRECT_EXECUTION` and fall back to per-section
  draws there (real iPhones and iPads have it).
- **The window**: on iOS winit's inner size is the safe area while the Metal layer fills the
  screen. `window::surface_size` (the outer size there) sizes the surface and `Resized`;
  `window::for_platform` drops the desktop window size and fullscreen, keeps landscape and defers
  the system's edge gestures.
- **No pointer to grab**: `capture` keeps the grab as the game's own state on iOS, as for
  offscreen runs; touch and a hardware mouse turn the view through `MouseHandler.onMove`.
- **Keyboard and mouse**: winit 0.30's iOS backend sends no hardware key presses (only
  `insertText:` while its view has the keyboard focus) and turns the pointer into touches. Keys
  come from GameController's `GCKeyboard`. The pointer is locked only while the game holds it
  (`capture`): winit's view controller gets a `prefersPointerLocked` method, and the game asks
  again if the system hasn't locked it within a second. In play, movement, buttons and the wheel
  come from `GCMouse`. Over screens they come from UIKit, on recognizers added to winit's view
  that hold no touch back from it: a `UIHoverGestureRecognizer` for hovering, one that sees only
  `UITouchTypeIndirectPointer` touches for presses, drags and releases (the event's
  `buttonMask` says which button; each moves the cursor to the event's place first, and those
  touches never reach the touch controls), and a `UIPanGestureRecognizer` that takes only
  scrolling. Pointer places reported while the pointer is locked are dropped: the system then
  still sends clicks as touches, at the window's corner. The in-play wheel assumes natural
  scrolling, iPadOS' default. `horizon-ios-input` builds winit's `KeyEvent` itself, which relies
  on its iOS `platform_specific` field being empty (a size check fails the build if a winit
  update changes that).
- **Main thread stack**: iOS gives the main thread 1 MB and the startup overflowed it;
  `crates/horizon-ios/build.rs` links the app with a 16 MB one.
- **Crates that don't build for iOS**: `arboard` (the clipboard stays in the game), the Vulkan
  HDR metadata (`horizon-hdr`, Vulkan only), `proc_pidinfo` (macOS only), winit's
  `key_without_modifiers` (no keyboard layouts to learn).
- **UIScene**: UIKit in the iOS 27 SDK refuses to launch an app without the UIScene life cycle,
  and an app linked against an older SDK runs boxed at old proportions on iPhone Duo's inner
  display. `third_party/winit` is winit 0.30.13 whose iOS windows join the connected
  `UIWindowScene` (`connected_window_scene` in `platform_impl/ios/window.rs`); `Info.plist` has
  a scene manifest. Drop the patch once a winit release adopts scenes.

## What's left

- Safe area: the HUD and screens draw under the Dynamic Island and the home indicator's corner.
- The deck: the hinge angle (`UIHingeInteraction`) to strengthen the reflection as the Duo folds,
  as Mirage does; moving items on the inventory page; more pages (chat, advancements).
- Touch: tuning (look speed, swipe thresholds), item drag-splitting in containers, a long press
  for the right click, chat.
- Keyboard and mouse: the edit shortcuts (copy, paste, select all) use Control as on Windows and
  Linux, not Command; the wheel in play doesn't follow the system's natural-scrolling setting;
  a screen opened from play keeps the cursor in the middle until the pointer moves or clicks
  (iPadOS can't move its pointer there); a button other than left or right may not reach screens
  if the system makes no touch for it; the startup menu (before the resources load) types with a
  US layout. Game controllers (`GCController`) aren't read yet.
- Memory and heat on devices: render distance and the worker count still use desktop defaults;
  watch for the system ending the app (jetsam) on phones with less memory.
- The narrator (`Unsupported platform ios`), the clipboard (UIPasteboard), Game Center, an app
  icon and a launch screen, an Xcode project for signing and TestFlight.
- Mods: native code mods are built by the launcher on the desktop; iOS can't load code at run time.
