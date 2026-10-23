#!/usr/bin/env bash
# Builds Spotifast for iPhone as an unsigned .ipa, for sideloading: AltStore,
# SideStore or Sideloadly sign it with the user's own Apple ID when they
# install it.
#
#   scripts/build-ipa.sh
#
# It runs on a Mac with Xcode 15 or newer and rustup, from any directory, and
# writes dist/Spotifast-<version>-ios-unsigned.ipa, where the version is the
# spotifast package's in Cargo.toml:
#
#   1. the app as a static library, libspotifast_ios.a (the spotifast-ios
#      package in ios/), built by cargo in release for aarch64-apple-ios;
#   2. the Xcode project in ios/ archived around it, without signing;
#   3. the archive's Spotifast.app put in Payload/ and zipped as the .ipa.
#
# BUNDLE_ID sets the bundle identifier (default com.placeholder.spotifast).
# The archive and Xcode's intermediate files go in build/ios/.
set -euo pipefail

die() {
    echo "build-ipa.sh: $*" >&2
    exit 1
}

step() {
    printf '\n==> %s\n' "$*"
}

# Everything this needs is checked first, so a missing tool stops the build
# before the long part.
[ "$(uname -s)" = Darwin ] || die "the iOS app is built with Xcode, so this runs on macOS only"
for tool in xcodebuild xcrun cargo rustup python3 ditto zip; do
    if ! command -v "$tool" > /dev/null; then
        case $tool in
            xcodebuild | xcrun) die "$tool is missing: install Xcode 15 or newer" ;;
            cargo | rustup) die "$tool is missing: install Rust with rustup (https://rustup.rs)" ;;
            *) die "$tool is missing: it comes with macOS and Xcode's command line tools" ;;
        esac
    fi
done
xcode=$(xcodebuild -version 2> /dev/null | awk '/^Xcode / { print $2 }')
[ -n "$xcode" ] || die "xcodebuild needs Xcode itself, not only the command line tools: install Xcode and select it with sudo xcode-select -s /Applications/Xcode.app"
[ "${xcode%%.*}" -ge 15 ] || die "Xcode $xcode is too old: the project needs Xcode 15 or newer"
xcrun --sdk iphoneos --show-sdk-path > /dev/null 2>&1 || die "Xcode $xcode has no iOS SDK: add the iOS platform in Xcode's settings, under Components"

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo"

# rust-toolchain.toml pins the toolchain, so the target's standard library
# must be in that toolchain, which rustup picks when run here.
target=aarch64-apple-ios
installed=$(rustup target list --installed 2> /dev/null || true)
if ! grep -qx "$target" <<< "$installed"; then
    step "Adding the $target standard library to the pinned toolchain"
    rustup target add "$target"
fi

# The version is the spotifast package's, found by name: the workspace also
# holds spotifast-ios, so the order of the package list means nothing.
metadata=$(cargo metadata --no-deps --format-version 1)
version=$(python3 -c '
import json, sys
print(next((p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "spotifast"), ""))
' <<< "$metadata")
target_dir=$(python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])' <<< "$metadata")
[ -n "$version" ] || die "cargo metadata lists no spotifast package"
# CFBundleVersion takes numbers only, so a pre-release's suffix (and any build
# metadata) comes off it, as in packaging/macos/bundle.sh.
build_number=${version%%[-+]*}

bundle_id=${BUNDLE_ID:-com.placeholder.spotifast}
[[ $bundle_id =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ ]] ||
    die "BUNDLE_ID '$bundle_id' is not a bundle identifier (letters, digits and hyphens between dots, like com.example.spotifast)"

lib_dir=$target_dir/$target/release
work=$repo/build/ios
archive=$work/Spotifast.xcarchive
ipa=$repo/dist/Spotifast-$version-ios-unsigned.ipa

# The minimum iOS the app declares, for the C code that crates such as ring
# compile too, so that every object in the library agrees with the app.
export IPHONEOS_DEPLOYMENT_TARGET=16.0

step "Rust: spotifast-ios for Spotifast $version, release, $target"
# spotifast-ios depends on spotifast without its default features, which
# leaves MilkDrop out: a C++ library with a window and an event loop of its
# own, which iOS cannot host.
packages=$(cargo tree --target "$target" -p spotifast-ios --edges normal --prefix none --format '{p}')
if grep -q '^projectm-sys ' <<< "$packages"; then
    die "MilkDrop (projectm-sys) is in the iOS build: ios/Cargo.toml must depend on spotifast with default-features = false"
fi
cargo build --release --target "$target" -p spotifast-ios
[ -f "$lib_dir/libspotifast_ios.a" ] || die "cargo left no libspotifast_ios.a in $lib_dir"

step "Xcode $xcode: archiving Spotifast.app ($bundle_id, $version), unsigned"
rm -rf "$archive"
# STRIP_INSTALLED_PRODUCT=NO keeps the symbols in the binary, so a panic's
# backtrace in the app's panic.log names its functions and a crash report
# can be symbolicated from the .ipa itself.
xcodebuild archive \
    -project ios/Spotifast.xcodeproj \
    -scheme Spotifast \
    -configuration Release \
    -sdk iphoneos \
    -destination 'generic/platform=iOS' \
    -archivePath "$archive" \
    -derivedDataPath "$work/DerivedData" \
    SPOTIFAST_RUST_LIB_DIR="$lib_dir" \
    CODE_SIGNING_ALLOWED=NO \
    CODE_SIGNING_REQUIRED=NO \
    PRODUCT_BUNDLE_IDENTIFIER="$bundle_id" \
    MARKETING_VERSION="$version" \
    CURRENT_PROJECT_VERSION="$build_number" \
    STRIP_INSTALLED_PRODUCT=NO
app=$archive/Products/Applications/Spotifast.app
[ -d "$app" ] || die "the archive holds no Products/Applications/Spotifast.app"

# The signing tools read the bundle id, versions and capabilities from the
# app's Info.plist, so every key of ios/Info.plist must have come through
# the archive, with its build settings filled in.
step "Info.plist"
python3 - "$repo/ios/Info.plist" "$app" "$bundle_id" "$version" "$build_number" << 'PY' ||
import os
import plistlib
import sys

source_path, app, bundle_id, version, build_number = sys.argv[1:]
with open(source_path, "rb") as file:
    source = plistlib.load(file)
with open(os.path.join(app, "Info.plist"), "rb") as file:
    built = plistlib.load(file)

problems = [f"{key} from ios/Info.plist is missing" for key in source if key not in built]


def unexpanded(value):
    if isinstance(value, str):
        return "$(" in value
    if isinstance(value, dict):
        return any(unexpanded(item) for item in value.values())
    if isinstance(value, list):
        return any(unexpanded(item) for item in value)
    return False


problems += [f"{key} still holds a build setting: {built[key]!r}" for key in built if unexpanded(built[key])]
expected = {
    "CFBundleIdentifier": bundle_id,
    "CFBundleShortVersionString": version,
    "CFBundleVersion": build_number,
    "MinimumOSVersion": "16.0",
}
problems += [f"{key} is {built.get(key)!r}, not {value!r}" for key, value in expected.items() if built.get(key) != value]
if "audio" not in built.get("UIBackgroundModes", []):
    problems.append("UIBackgroundModes lacks audio, which keeps playback going in the background")
executable = built.get("CFBundleExecutable", "")
if not executable or not os.path.isfile(os.path.join(app, executable)):
    problems.append(f"the executable {executable!r} is not in the app")
icon = built.get("CFBundleIcons", {}).get("CFBundlePrimaryIcon") or built.get("CFBundleIconName")
if not os.path.isfile(os.path.join(app, "Assets.car")) or not icon:
    problems.append("the app icon is missing (Assets.car and CFBundleIcons come from ios/Assets.xcassets)")

for key in [*expected, "CFBundleExecutable", "UIBackgroundModes", "UIDeviceFamily"]:
    print(f"    {key}: {built.get(key)}")
print(f"    app icon: {icon}")
for problem in problems:
    print(f"build-ipa.sh: {problem}", file=sys.stderr)
sys.exit(1 if problems else 0)
PY
    die "the app's Info.plist is not what ios/Info.plist and the build settings make"

step "Executable"
binary=$app/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Info.plist")
echo "    architectures: $(lipo -archs "$binary")"
echo "    build version: $(xcrun vtool -show-build "$binary" 2>&1 | grep -E 'platform|minos|sdk' | xargs)"
echo "    symbols: $(nm "$binary" 2> /dev/null | wc -l | xargs)"
# Entitlements live in the code signature, and an unsigned app has none, so
# the Keychain group in ios/Spotifast.entitlements is not in the .ipa: the
# signing tool gives the app the entitlements its provisioning profile allows.
if codesign --display "$app" > /dev/null 2>&1; then
    echo "    signature: $(codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Signature|CodeDirectory)' | tr '\n' ' ')"
    entitlements=$(codesign --display --entitlements - --xml "$app" 2> /dev/null || true)
    echo "    entitlements: ${entitlements:-none}"
else
    echo "    signature: none (the signing tool signs it when installing it); entitlements: none"
fi

step "Packaging"
rm -rf "$work/ipa"
mkdir -p "$work/ipa/Payload" "$repo/dist"
ditto "$app" "$work/ipa/Payload/Spotifast.app"
# zip adds to an existing archive, so a stale one goes first.
rm -f "$ipa"
(cd "$work/ipa" && zip -qry -X "$ipa" Payload)

bytes=$(wc -c < "$ipa" | xargs)
echo "$ipa"
echo "$bytes bytes ($(awk -v bytes="$bytes" 'BEGIN { printf "%.1f", bytes / 1048576 }') MiB)"
