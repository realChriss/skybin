#!/bin/sh
set -eu

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	bold='\033[1m' dim='\033[2m' red='\033[31m' green='\033[32m' reset='\033[0m'
else
	bold='' dim='' red='' green='' reset=''
fi
ok() { printf "  ${green}✓${reset} %b\n" "$1"; }
fail() {
	printf "  ${red}✗${reset} %b\n\n" "$1" >&2
	exit 1
}
download() {
	if command -v curl >/dev/null; then
		curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null; then
		wget -qO "$2" "$1"
	else
		fail "Needs curl or wget."
	fi
}

printf "\n  ${bold}Skybin${reset} installer\n\n"

case $(uname -s) in
Linux) os=linux ;;
Darwin) os=macos ;;
*) fail "Unsupported OS: $(uname -s)" ;;
esac
case $(uname -m) in
x86_64 | amd64) arch=x86_64 ;;
arm64 | aarch64) arch=aarch64 ;;
*) fail "Unsupported CPU: $(uname -m)" ;;
esac
case $os-$arch in
linux-x86_64 | macos-aarch64) ;;
*) fail "No prebuilt binary for $os-$arch. Build it from source: https://github.com/realChriss/skybin#build-from-source" ;;
esac

repo=https://github.com/realChriss/skybin
raw=https://raw.githubusercontent.com/realChriss/skybin/main
if [ "$os" = macos ]; then
	app=$HOME/Applications/Skybin.app
	bin=$app/Contents/MacOS/skybin
else
	bin=$HOME/.local/bin/skybin
fi

mkdir -p "${bin%/*}"
tmp=$bin.new
trap 'rm -f "$tmp"' EXIT
download "$repo/releases/latest/download/skybin-$os-$arch" "$tmp" || fail "Download failed: $repo/releases/latest/download/skybin-$os-$arch"
ok "Downloaded skybin-$os-$arch ${dim}($(du -h "$tmp" | cut -f1))${reset}"
chmod 755 "$tmp"
mv "$tmp" "$bin"

if [ "$os" = macos ]; then
	mkdir -p "$app/Contents/Resources"
	download "$raw/assets/icon.icns" "$app/Contents/Resources/icon.icns" || fail "Icon download failed"
	cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>Skybin</string>
	<key>CFBundleIdentifier</key><string>io.github.realchriss.skybin</string>
	<key>CFBundleExecutable</key><string>skybin</string>
	<key>CFBundleIconFile</key><string>icon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
	# make Launchpad and Spotlight pick up the new bundle right away
	lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
	[ -x "$lsregister" ] && "$lsregister" -f "$app" || touch "$app"
	ok "Installed to ~/Applications/Skybin.app"
	printf "\n  Open ${bold}Skybin${reset} from Launchpad or Spotlight.\n\n"
else
	ok "Installed to ~/.local/bin/skybin"
	share=${XDG_DATA_HOME:-$HOME/.local/share}
	icon=$share/icons/hicolor/256x256/apps/skybin.png
	mkdir -p "${icon%/*}" "$share/applications"
	download "$raw/assets/icon.png" "$icon" || fail "Icon download failed"
	# the file name and StartupWMClass match the window's app id, which ties the window to this entry
	cat >"$share/applications/skybin.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Skybin
Comment=Manage S3 buckets
Exec=$bin
Icon=$icon
StartupWMClass=skybin
Categories=Utility;Network;
EOF
	ok "Added to the app menu"
	printf "\n  Open ${bold}Skybin${reset} from your app menu.\n\n"
fi
