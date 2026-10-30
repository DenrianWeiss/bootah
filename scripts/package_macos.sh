#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
python3 scripts/prepare_boot_assets.py
cargo build --release
swift build -c release --package-path apps/macos
app="$PWD/.artifacts/Bootah.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/netboot-core "$app/Contents/MacOS/netboot-core"
cp target/release/netboot-tui "$app/Contents/MacOS/netboot-tui"
sh scripts/copy_package_assets.sh "$app/Contents/MacOS/assets" macos
cp apps/macos/.build/release/Netboot "$app/Contents/MacOS/Bootah"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Bootah</string>
<key>CFBundleIdentifier</key><string>work.premutation.bootah</string>
<key>CFBundleName</key><string>Bootah</string>
<key>CFBundleIconFile</key><string>bootah.icns</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSLocalNetworkUsageDescription</key><string>Provide Netboot For Local Machines</string>
</dict></plist>
PLIST
python3 scripts/generate_versions.py --platform plist --output "$app/Contents/Info.plist"
cp assets/branding/bootah.icns "$app/Contents/Resources/bootah.icns"
mkdir -p "$app/Contents/Resources/system-icons"
cp -R resources/system-icons/light resources/system-icons/dark "$app/Contents/Resources/system-icons/"
cp -R LICENSES "$app/Contents/Resources/"
mkdir -p "$app/Contents/Library/LaunchDaemons"
cp packaging/macos/work.premutation.bootah.network.plist "$app/Contents/Library/LaunchDaemons/"
identity="${BOOTAH_SIGN_IDENTITY:--}"
if [ "$identity" = - ]; then
    codesign --force --deep --options runtime --sign - "$app"
else
    codesign --force --identifier work.premutation.bootah.network-core --options runtime --timestamp --sign "$identity" "$app/Contents/MacOS/netboot-core"
    codesign --force --identifier work.premutation.bootah.network-core --options runtime --timestamp --sign "$identity" "$app/Contents/MacOS/netboot-tui"
    codesign --force --deep --preserve-metadata=identifier --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --deep --strict "$app"
printf '%s\n' "$app"

mkdir -p release
ditto -c -k --sequesterRsrc --keepParent "$app" "release/Bootah-macos-$(uname -m).zip"
python3 scripts/update_package_checksums.py
