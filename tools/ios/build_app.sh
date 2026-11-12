#!/bin/sh
# Builds Horizon.app for the iOS Simulator (default) or a device, and installs it on a
# simulator when asked. Dev tool; the resources are the ones horizon-prepare made from the
# player's own Minecraft (.local/horizon-client/26.3), never committed.
#
#   tools/ios/build_app.sh                     # simulator build, app in target/ios/Horizon.app
#   tools/ios/build_app.sh --install UDID      # and install it on that simulator
#   tools/ios/build_app.sh --device --sign "Apple Development: ..." --profile path.mobileprovision \
#       [--entitlements path.entitlements]
#
# The app's GameData is staged small (tools/ios/stage_resources.py): the resources packed into
# one zstd-compressed file, English alone of vanilla's languages (the app fetches the others),
# the sounds re-encoded. A build with a profile other than fast or dev ships a stripped binary;
# the unstripped one stays beside the app (horizon-ios.unstripped) to symbolicate crash reports
# with `atos -o`.
#
# Env: PROFILE (cargo profile, default fast), RESOURCES (default .local/horizon-client/26.3),
#      BUNDLE_ID (default dev.horizonlab.horizon.ios), BUILD_NUMBER (CFBundleVersion, default 1),
#      SOUND_QUALITY (oggenc -q, default 0), LANGUAGES (default en_us), NO_SOUNDS=1 to leave
#      the sounds out, HORIZON_MSA_CLIENT_ID (the Azure app the launcher's Microsoft sign-in
#      uses: Horizon's own builds only, docs/forking.md; without it the sign-in is off).
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
PROFILE=${PROFILE:-fast}
RESOURCES=${RESOURCES:-$ROOT/.local/horizon-client/26.3}
BUNDLE_ID=${BUNDLE_ID:-dev.horizonlab.horizon.ios}
TARGET=aarch64-apple-ios-sim
INSTALL=""
SIGN="-"
PROVISION=""
ENTITLEMENTS=""
while [ $# -gt 0 ]; do
    case "$1" in
        --device) TARGET=aarch64-apple-ios ;;
        --install) INSTALL=$2; shift ;;
        --sign) SIGN=$2; shift ;;
        --profile) PROVISION=$2; shift ;;
        --entitlements) ENTITLEMENTS=$2; shift ;;
        *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done
export PATH="$HOME/.cargo/bin:$PATH"
if [ "$TARGET" = aarch64-apple-ios ]; then
    export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
    MIN_KEY=iphoneos
else
    export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
    MIN_KEY=iphonesimulator
fi
cargo build --profile "$PROFILE" --target "$TARGET" -p horizon-ios

OUT="$ROOT/target/ios/$TARGET"
APP="$OUT/Horizon.app"
mkdir -p "$APP"
BINARY="target/$TARGET/$PROFILE/horizon-ios"
case "$PROFILE" in
    fast | dev) cp "$BINARY" "$APP/horizon-ios" ;;
    *)
        # Symbols only name functions in a crash report: they stay beside the app, not in it.
        cp "$BINARY" "$OUT/horizon-ios.unstripped"
        xcrun strip -x -S -o "$APP/horizon-ios" "$BINARY"
        ;;
esac
sed -e "s/@BUNDLE_ID@/$BUNDLE_ID/" -e "s/@MIN_OS@/$IPHONEOS_DEPLOYMENT_TARGET/" \
    -e "s/@PLATFORM@/$( [ "$MIN_KEY" = iphoneos ] && echo iPhoneOS || echo iPhoneSimulator )/" \
    "$ROOT/crates/horizon-ios/Info.plist" > "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${BUILD_NUMBER:-1}" "$APP/Info.plist"
python3 "$ROOT/tools/ios/stage_resources.py" --resources "$RESOURCES" --out "$APP/GameData" \
    --quality "${SOUND_QUALITY:-0}" --languages "${LANGUAGES:-en_us}"
if [ "${NO_SOUNDS:-0}" = 1 ]; then
    find "$APP/GameData" -name '*.ogg' -delete
fi
if [ -n "$PROVISION" ]; then
    cp "$PROVISION" "$APP/embedded.mobileprovision"
fi
if [ -n "$ENTITLEMENTS" ]; then
    codesign --force --sign "$SIGN" --timestamp=none --entitlements "$ENTITLEMENTS" "$APP"
else
    codesign --force --sign "$SIGN" --timestamp=none "$APP"
fi
echo "built $APP ($(du -sh "$APP" | cut -f1))"
if [ -n "$INSTALL" ]; then
    xcrun simctl install "$INSTALL" "$APP"
    echo "installed $BUNDLE_ID on $INSTALL"
fi
