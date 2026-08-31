#!/usr/bin/env bash
# Build the iOS app: the Rust core first, then the Xcode project that links it.
#
#   ./build.sh              # simulator build, and boot it
#   ./build.sh device       # device build (needs a signing team; see README)
#   ./build.sh core         # just the Rust staticlibs
#   ./build.sh project      # just regenerate AdiFleet.xcodeproj from project.yml
#   ./build.sh --regen-icon # redraw the app icon from the shared Trefoil geometry
#
# The Rust half is always built in release. See project.yml for why.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
scheme="AdiFleet"
# What the app is run on when no device is named. Any booted simulator would do; naming one keeps
# the run reproducible.
sim_name="${ADI_IOS_SIM:-iPhone 17 Pro}"

# The two Rust targets. The simulator one is separate from the device one on Apple silicon: same
# architecture, different ABI, and linking the wrong one fails in a way that reads like a missing
# symbol rather than a wrong platform.
sim_target="aarch64-apple-ios-sim"
device_target="aarch64-apple-ios"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

need() {
	command -v "$1" >/dev/null 2>&1 || {
		echo "error: $1 is not installed" >&2
		exit 1
	}
}

build_core() {
	local target="$1"
	log "building the mesh core for $target"
	# Only this crate. Building the workspace here would also build adi-hive's binary, and on a
	# development machine the running front door watches its own binary and restarts when it
	# changes — a rebuild triggered by an iOS build would cost ~40s of *.adi downtime.
	(cd "$repo" && cargo build --release -p adi-mesh-ffi --target "$target")
}

generate_project() {
	need xcodegen
	log "generating AdiFleet.xcodeproj"
	(cd "$here" && xcodegen generate --quiet)
}

case "${1:-simulator}" in
core)
	build_core "$sim_target"
	build_core "$device_target"
	;;

project)
	generate_project
	;;

--regen-icon)
	# Redraw AppIcon-1024.png from the *current* Trefoil geometry. This exists because the icon
	# silently rotted once: it was exported by hand in Aug 2026 and still showed a wireframe
	# hexagon from before the Trefoil, months after apps/macos regenerated its own. Nothing in
	# the build looked at it, so nothing noticed. Now one command redraws it from the same
	# `Sources/Trefoil.swift` the Mac app and the .adi error pages draw from.
	need swiftc
	log "regenerating AppIcon-1024.png"
	tmp="$(mktemp -d)"
	trap 'rm -rf "$tmp"' EXIT
	swiftc -parse-as-library -O \
		"$repo/apps/macos/Sources/Trefoil.swift" "$repo/apps/macos/icon-gen.swift" \
		-o "$tmp/icon-gen"
	# --ios: full-bleed and opaque. The system applies the corner mask, and App Store Connect
	# rejects an icon carrying an alpha channel (ITMS-90717).
	"$tmp/icon-gen" --ios "$here/AdiFleet/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"
	log "wrote AdiFleet/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"
	;;

device)
	need xcodebuild
	build_core "$device_target"
	generate_project

	# The team baked into project.yml, unless one is named here. xcodegen writes settings
	# verbatim — it does not expand `${VAR}` — so the override belongs on the command line,
	# where a build setting outranks the project's own.
	team_arg=()
	[ -n "${DEVELOPMENT_TEAM:-}" ] && team_arg=("DEVELOPMENT_TEAM=$DEVELOPMENT_TEAM")

	log "building and signing for a device"
	# `-target`, not `-scheme`. A scheme build resolves a destination first, and this Xcode has
	# the iOS SDK without the iOS *platform* component — so it calls the connected iPhone
	# ineligible ("iOS 26.5 is not installed") and refuses before compiling anything. Building the
	# target names the SDK directly, never enumerates destinations, and produces the same signed
	# .app; devicectl installs it below without Xcode's device machinery either. The alternative
	# is a multi-gigabyte `xcodebuild -downloadPlatform iOS`, which buys nothing here.
	#
	# `-allowProvisioningUpdates` lets Xcode register the device and mint the profile itself,
	# which is what makes this one command rather than a trip through the UI.
	xcodebuild \
		-project "$here/$scheme.xcodeproj" \
		-target "$scheme" \
		-configuration Debug \
		-sdk iphoneos \
		-allowProvisioningUpdates \
		"${team_arg[@]}" \
		CONFIGURATION_BUILD_DIR="$here/.build-device/Products" \
		build

	app="$here/.build-device/Products/$scheme.app"
	device="${ADI_IOS_DEVICE:-}"
	if [ -z "$device" ]; then
		# Matched by the shape of the identifier, not by column: the model column holds a variable
		# number of words ("iPhone 14 Pro Max (iPhone15,3)"), so counting fields from either end
		# picks a different token per device.
		device="$(xcrun devicectl list devices 2>/dev/null | grep -m1 available |
			grep -oE '[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}' | head -1)"
	fi
	[ -n "$device" ] || {
		echo "error: no paired device found; connect one or set ADI_IOS_DEVICE=<identifier>" >&2
		exit 1
	}

	log "installing on $device"
	xcrun devicectl device install app --device "$device" "$app"
	xcrun devicectl device process launch --device "$device" family.adi.fleet
	log "installed and launched. A device that has never run a development build from this team"
	log "needs Settings → General → VPN & Device Management → Trust, once."
	;;

simulator | *)
	need xcodebuild
	build_core "$sim_target"
	generate_project

	log "booting the $sim_name simulator"
	# `boot` on an already-booted device exits non-zero; that is the state we want either way.
	udid="$(xcrun simctl list devices available | grep -m1 "$sim_name (" | sed -E 's/.*\(([-0-9A-F]{36})\).*/\1/')"
	[ -n "$udid" ] || {
		echo "error: no simulator called '$sim_name'" >&2
		exit 1
	}
	xcrun simctl boot "$udid" 2>/dev/null || true
	open -a Simulator

	log "building"
	# `-sdk iphonesimulator` rather than `-destination id=…`: this Xcode enumerates no simulator
	# destinations at all (it reports only device ones, and calls those ineligible because the iOS
	# *platform* component is not installed), so destination resolution fails before the build
	# starts. Naming the SDK sidesteps the enumeration entirely and produces the same product,
	# which simctl then installs by UDID below.
	xcodebuild \
		-project "$here/$scheme.xcodeproj" \
		-scheme "$scheme" \
		-configuration Debug \
		-sdk iphonesimulator \
		-derivedDataPath "$here/.build" \
		build

	app="$here/.build/Build/Products/Debug-iphonesimulator/$scheme.app"
	log "installing and launching"
	xcrun simctl install "$udid" "$app"
	xcrun simctl launch --console-pty "$udid" family.adi.fleet
	;;
esac
