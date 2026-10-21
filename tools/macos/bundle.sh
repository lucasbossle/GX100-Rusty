#!/bin/sh
# Builds GX100-Rusty.app (universal: Apple Silicon + Intel) and a zip of it in dist/.
# Needs: rustup target add aarch64-apple-darwin x86_64-apple-darwin
# The app contains no Roland data: it reads the tables from BOSS TONE STUDIO at first launch.
set -eu
cd "$(dirname "$0")/../.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/gx100-gui/Cargo.toml | head -1)
app=dist/GX100-Rusty.app
export MACOSX_DEPLOYMENT_TARGET=11.0

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p gx100-gui --target "$target"
done

rm -rf "$app" "dist/GX100-Rusty-$version-macos.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/GX100-Rusty" \
  target/aarch64-apple-darwin/release/gx100-rusty target/x86_64-apple-darwin/release/gx100-rusty

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>GX100-Rusty</string>
  <key>CFBundleDisplayName</key><string>GX100-Rusty</string>
  <key>CFBundleIdentifier</key><string>io.github.lucasbossle.gx100-rusty</string>
  <key>CFBundleExecutable</key><string>GX100-Rusty</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT OR Apache-2.0. Not affiliated with Roland or BOSS.</string>
</dict>
</plist>
EOF

# Ad-hoc signature (no Apple Developer ID): macOS will ask the user to allow it once.
codesign --force --deep --sign - "$app"
(cd dist && ditto -c -k --keepParent GX100-Rusty.app "GX100-Rusty-$version-macos.zip")
lipo -info "$app/Contents/MacOS/GX100-Rusty"
echo "built dist/GX100-Rusty-$version-macos.zip"
