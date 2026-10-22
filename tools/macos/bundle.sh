#!/bin/sh
# Builds OpenGX100 Editor.app (universal: Apple Silicon + Intel) and a zip of it in dist/.
# Needs: rustup target add aarch64-apple-darwin x86_64-apple-darwin
# The app contains no Roland data: it reads the tables from BOSS TONE STUDIO at first launch.
set -eu
cd "$(dirname "$0")/../.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/opengx100-gui/Cargo.toml | head -1)
app="dist/OpenGX100 Editor.app"
export MACOSX_DEPLOYMENT_TARGET=11.0

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p opengx100-gui --target "$target"
done

rm -rf "$app" "dist/OpenGX100-Editor-$version-macos.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/OpenGX100 Editor" \
  target/aarch64-apple-darwin/release/opengx100-editor target/x86_64-apple-darwin/release/opengx100-editor

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>OpenGX100 Editor</string>
  <key>CFBundleDisplayName</key><string>OpenGX100 Editor</string>
  <key>CFBundleIdentifier</key><string>io.github.lucasbossle.opengx100-editor</string>
  <key>CFBundleExecutable</key><string>OpenGX100 Editor</string>
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
(cd dist && ditto -c -k --keepParent "OpenGX100 Editor.app" "OpenGX100-Editor-$version-macos.zip")
lipo -info "$app/Contents/MacOS/OpenGX100 Editor"
echo "built dist/OpenGX100-Editor-$version-macos.zip"
