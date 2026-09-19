#!/usr/bin/env bash
# PIC - Picasa iPhoto Clone Linux packager
# Builds AppImage and/or Flatpak from either local files (fully offline) or latest GitHub source.
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
ORIGINAL_ARGS=("$@")
CACHE_ROOT="${PIC_BUILD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/pic-linux-build}"
TOOLS_DIR="$CACHE_ROOT/tools"
GITHUB_CACHE="$CACHE_ROOT/github-source"
WORK_ROOT="$CACHE_ROOT/work"
DIST_DIR="${PIC_DIST_DIR:-$SCRIPT_DIR/dist}"
REPO_URL="${PIC_REPO_URL:-https://github.com/froggy744/picasa-iphoto-clone.git}"
DEFAULT_BRANCH="${PIC_BRANCH:-main}"
APP_ID="${PIC_APP_ID:-io.github.you.PicasaRs}"
BIN_NAME_OVERRIDE="${PIC_BIN_NAME:-}"
BIN_NAME=""
GNOME_RUNTIME="${PIC_GNOME_RUNTIME:-50}"
FDO_RUST_RUNTIME="${PIC_FDO_RUST_RUNTIME:-25.08}"
MODE=""
PROJECT_DIR=""
BRANCH="$DEFAULT_BRANCH"
ONLINE=0
SKIP_TESTS="${PIC_SKIP_TESTS:-0}"
STRICT_TESTS="${PIC_STRICT_TESTS:-0}"
BUILD_TARGET="${PIC_BUILD_TARGET:-}"
LOG_DIR="${PIC_BUILD_LOG_DIR:-$SCRIPT_DIR/build-logs}"
LOG_FILE=""
BUILD_STARTED_AT=""
CANCEL_SIGNAL=""

log()  { printf '\n\033[1;34m==>\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[1;32mOK:\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mWARN:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'HELP'
PIC Linux build script

Build targets:
  • Both AppImage + Flatpak (default)
  • AppImage only
  • Flatpak only

Source modes:
  local    Build the files already on this PC. OFFLINE: no fetch, pull or download.
  github   Clone/update the latest GitHub branch, cache build requirements, then build.

Interactive:
  ./build-linux.sh

Direct commands:
  ./build-linux.sh local
  ./build-linux.sh local --project /home/peet/picasa-clone
  ./build-linux.sh github
  ./build-linux.sh github --branch main
  ./build-linux.sh github --branch editing.phase1
  ./build-linux.sh local --appimage-only
  ./build-linux.sh local --flatpak-only
  ./build-linux.sh local --target appimage

Options:
  --source MODE       local or github
  --project PATH      local project folder (default: folder containing this script)
  --branch NAME       GitHub branch (default: main)
  --dist PATH         output folder (default: ./dist beside this script)
  --log-dir PATH      build log folder (default: ./build-logs beside this script)
  --target TARGET     both, appimage, or flatpak (default: both)
  --appimage-only     build only the AppImage
  --flatpak-only      build only the Flatpak bundle
  --strict-tests      stop packaging if cargo test fails
  --skip-tests        do not run cargo test
  -h, --help          show this help

Useful environment overrides:
  PIC_SKIP_TESTS=1             skip cargo test
  PIC_STRICT_TESTS=1           make test failures fatal
  PIC_GNOME_RUNTIME=50         Flatpak GNOME runtime branch
  PIC_FDO_RUST_RUNTIME=25.08   Flatpak Rust SDK-extension branch
  PIC_APP_ID=...               application/Flatpak ID
  PIC_BUILD_CACHE=...          build cache location
  PIC_BUILD_LOG_DIR=...        build log folder
  PIC_BUILD_TARGET=...         both, appimage, or flatpak

Offline rule:
  'local' mode never uses git fetch/pull/clone, curl, wget, or Flatpak downloads.
  Required Rust crates, Flatpak runtimes/SDKs, and linuxdeploy must already be cached/
  installed. Run GitHub mode once while online to prepare these automatically.
HELP
}

while (($#)); do
    case "$1" in
        local|github)
            [[ -z "$MODE" ]] || die "Source mode specified more than once."
            MODE="$1"; shift ;;
        --source)
            [[ $# -ge 2 ]] || die "--source needs local or github"
            MODE="$2"; shift 2 ;;
        --project)
            [[ $# -ge 2 ]] || die "--project needs a path"
            PROJECT_DIR="$2"; shift 2 ;;
        --branch)
            [[ $# -ge 2 ]] || die "--branch needs a branch name"
            BRANCH="$2"; shift 2 ;;
        --dist)
            [[ $# -ge 2 ]] || die "--dist needs a path"
            DIST_DIR="$2"; shift 2 ;;
        --log-dir)
            [[ $# -ge 2 ]] || die "--log-dir needs a path"
            LOG_DIR="$2"; shift 2 ;;
        --target)
            [[ $# -ge 2 ]] || die "--target needs both, appimage, or flatpak"
            [[ -z "$BUILD_TARGET" ]] || die "Build target specified more than once."
            BUILD_TARGET="$2"; shift 2 ;;
        --appimage-only)
            [[ -z "$BUILD_TARGET" ]] || die "Build target specified more than once."
            BUILD_TARGET=appimage; shift ;;
        --flatpak-only)
            [[ -z "$BUILD_TARGET" ]] || die "Build target specified more than once."
            BUILD_TARGET=flatpak; shift ;;
        --strict-tests)
            STRICT_TESTS=1; shift ;;
        --skip-tests)
            SKIP_TESTS=1; shift ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            die "Unknown argument: $1 (use --help)" ;;
    esac
done

start_logging() {
    mkdir -p "$LOG_DIR"
    LOG_DIR="$(cd -- "$LOG_DIR" && pwd -P)"
    local stamp
    stamp="$(date '+%Y-%m-%d-%H%M%S')"
    LOG_FILE="$LOG_DIR/build-${stamp}-$$.log"
    : > "$LOG_FILE"

    # Stream all subsequent stdout/stderr to both the terminal and the log.
    # The file is written continuously, so it remains useful if the build is
    # interrupted before AppImage/Flatpak packaging finishes.
    # Keep the logger alive through Ctrl+C/TERM so the cancellation footer can
    # still be written. It exits naturally when this script closes the pipe.
    exec > >(trap '' INT TERM HUP; exec tee -a "$LOG_FILE") 2>&1

    BUILD_STARTED_AT="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    printf '%s\n' '============================================================'
    printf '%s\n' 'PIC Linux Packager build log'
    printf 'Started: %s\n' "$BUILD_STARTED_AT"
    printf 'PID:     %s\n' "$$"
    printf 'Script:  %s\n' "$0"
    printf 'Command:'
    printf ' %q' "$0" "${ORIGINAL_ARGS[@]}"
    printf '\n'
    printf '%s\n' '============================================================'
}

handle_signal() {
    local signal="$1" code="$2"
    CANCEL_SIGNAL="$signal"
    printf '\n%s\n' '============================================================'
    printf '%s\n' 'BUILD CANCELLED'
    printf 'Signal: %s\n' "$signal"
    printf 'Time:   %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')"
    printf '%s\n' '============================================================'
    exit "$code"
}

finish_logging() {
    local status=$?
    trap - EXIT
    local finished outcome
    finished="$(date '+%Y-%m-%dT%H:%M:%S%z')"

    if [[ -n "$CANCEL_SIGNAL" ]]; then
        outcome="CANCELLED"
    elif ((status == 130)); then
        CANCEL_SIGNAL="INT"
        outcome="CANCELLED"
    elif ((status == 143)); then
        CANCEL_SIGNAL="TERM"
        outcome="CANCELLED"
    elif ((status == 0)); then
        outcome="SUCCESS"
    else
        outcome="FAILED"
    fi

    printf '\n%s\n' '============================================================'
    printf 'Build session: %s\n' "$outcome"
    printf 'Finished:      %s\n' "$finished"
    printf 'Exit code:     %s\n' "$status"
    [[ -z "$CANCEL_SIGNAL" ]] || printf 'Signal:        %s\n' "$CANCEL_SIGNAL"
    printf 'Build log: %s\n' "$LOG_FILE"
    printf '%s\n' '============================================================'
    return "$status"
}

start_logging
trap 'handle_signal INT 130' INT
trap 'handle_signal TERM 143' TERM
trap finish_logging EXIT

interactive_menu() {
    printf '\nPIC Linux Packager\n'
    printf '  1) Local files  (OFFLINE - no internet used)\n'
    printf '  2) GitHub latest\n'
    printf '  3) Exit\n\n'
    read -r -p 'Choose [1-3]: ' choice
    case "$choice" in
        1)
            MODE=local
            read -r -p "Project folder [$SCRIPT_DIR]: " PROJECT_DIR
            PROJECT_DIR="${PROJECT_DIR:-$SCRIPT_DIR}"
            ;;
        2)
            MODE=github
            read -r -p "Git branch [$DEFAULT_BRANCH]: " BRANCH
            BRANCH="${BRANCH:-$DEFAULT_BRANCH}"
            ;;
        3) exit 0 ;;
        *) die "Invalid choice." ;;
    esac

    if [[ -z "$BUILD_TARGET" ]]; then
        printf '\nWhat do you want to build?\n'
        printf '  1) Both AppImage + Flatpak\n'
        printf '  2) AppImage only\n'
        printf '  3) Flatpak only\n\n'
        read -r -p 'Choose [1-3]: ' target_choice
        case "$target_choice" in
            1|'') BUILD_TARGET=both ;;
            2) BUILD_TARGET=appimage ;;
            3) BUILD_TARGET=flatpak ;;
            *) die "Invalid build target." ;;
        esac
    fi
}

[[ -n "$MODE" ]] || interactive_menu
[[ "$MODE" == local || "$MODE" == github ]] || die "Source mode must be local or github."
BUILD_TARGET="${BUILD_TARGET:-both}"
[[ "$BUILD_TARGET" == both || "$BUILD_TARGET" == appimage || "$BUILD_TARGET" == flatpak ]] || \n    die "Build target must be both, appimage, or flatpak."

mkdir -p "$CACHE_ROOT" "$TOOLS_DIR" "$WORK_ROOT" "$DIST_DIR"

have() { command -v "$1" >/dev/null 2>&1; }

fedora_hint() {
    cat >&2 <<'HINT'

On Fedora, the usual build prerequisites are:
  sudo dnf install -y cargo rust git gtk4-devel libadwaita-devel flatpak flatpak-builder \
      cmake gcc gcc-c++ make pkgconf-pkg-config file patchelf nasm curl tar ImageMagick

Then run this script again.
HINT
}

check_host_tools() {
    local missing=()
    local commands=(cargo rustc pkg-config cmake cc make file tar)
    if [[ "$BUILD_TARGET" == both || "$BUILD_TARGET" == flatpak ]]; then
        commands+=(flatpak flatpak-builder)
    fi
    [[ "$MODE" == github ]] && commands+=(git)
    for cmd in "${commands[@]}"; do
        have "$cmd" || missing+=("$cmd")
    done
    if ((${#missing[@]})); then
        printf 'Missing commands: %s\n' "${missing[*]}" >&2
        fedora_hint
        exit 1
    fi

    if ! pkg-config --exists 'gtk4 >= 4.12' 'libadwaita-1 >= 1.5'; then
        printf 'GTK4/libadwaita development packages are missing or too old.\n' >&2
        fedora_hint
        exit 1
    fi
}

validate_project() {
    local dir="$1"
    [[ -d "$dir" ]] || die "Project directory does not exist: $dir"
    [[ -f "$dir/Cargo.toml" ]] || die "Cargo.toml not found in: $dir"
    [[ -f "$dir/Cargo.lock" ]] || die "Cargo.lock not found. Commit/generate Cargo.lock first for repeatable offline builds."
}

prepare_local_source() {
    ONLINE=0
    PROJECT_DIR="${PROJECT_DIR:-$SCRIPT_DIR}"
    PROJECT_DIR="$(cd -- "$PROJECT_DIR" && pwd -P)"
    validate_project "$PROJECT_DIR"
    SOURCE_DIR="$PROJECT_DIR"
    log "LOCAL/OFFLINE source selected"
    printf 'Source: %s\n' "$SOURCE_DIR"
    printf 'Network: DISABLED by this mode\n'
    if [[ -d "$SOURCE_DIR/.git" ]] && have git; then
        local branch dirty
        branch="$(git -C "$SOURCE_DIR" branch --show-current 2>/dev/null || true)"
        dirty="$(git -C "$SOURCE_DIR" status --porcelain 2>/dev/null || true)"
        printf 'Git branch: %s\n' "${branch:-detached/unknown}"
        [[ -z "$dirty" ]] || warn "Local tree has uncommitted changes. They WILL be included in this build."
    fi
}

prepare_github_source() {
    ONLINE=1
    check_host_tools
    log "GitHub latest source selected"
    printf 'Repository: %s\nBranch: %s\n' "$REPO_URL" "$BRANCH"

    if [[ -d "$GITHUB_CACHE/.git" ]]; then
        log "Updating clean cached GitHub checkout"
        git -C "$GITHUB_CACHE" remote set-url origin "$REPO_URL"
        git -C "$GITHUB_CACHE" fetch --prune --depth 1 origin "$BRANCH"
        git -C "$GITHUB_CACHE" reset --hard FETCH_HEAD
        # Keep ignored Cargo target/cache directories; remove only untracked non-ignored files.
        git -C "$GITHUB_CACHE" clean -fd
    else
        rm -rf "$GITHUB_CACHE"
        git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$GITHUB_CACHE"
    fi

    SOURCE_DIR="$GITHUB_CACHE"
    validate_project "$SOURCE_DIR"

    # Prime Cargo's normal cache now. Every actual build below uses --offline.
    log "Caching Rust dependencies for future offline builds"
    (cd "$SOURCE_DIR" && cargo fetch --locked)
}

download_file() {
    local url="$1" dest="$2"
    if have curl; then
        curl -fL --retry 3 --connect-timeout 20 -o "$dest.tmp" "$url"
    elif have wget; then
        wget -O "$dest.tmp" "$url"
    else
        die "Need curl or wget to download $url"
    fi
    mv -f "$dest.tmp" "$dest"
}

linuxdeploy_path() {
    local machine tool arch_url
    machine="$(uname -m)"
    case "$machine" in
        x86_64|amd64) arch_url=x86_64 ;;
        i386|i486|i586|i686) arch_url=i386 ;;
        *)
            if have linuxdeploy; then command -v linuxdeploy; return 0; fi
            warn "AppImage skipped: automatic linuxdeploy download supports x86_64/i386 here. Install linuxdeploy manually for $machine."
            return 1
            ;;
    esac

    tool="$TOOLS_DIR/linuxdeploy-${arch_url}.AppImage"
    if [[ ! -x "$tool" ]]; then
        if ((ONLINE)); then
            log "Caching linuxdeploy (one-time online setup)"
            if ! download_file \
                "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${arch_url}.AppImage" \
                "$tool"; then
                warn "AppImage skipped: linuxdeploy could not be downloaded."
                return 1
            fi
            chmod +x "$tool"
        elif have linuxdeploy; then
            command -v linuxdeploy
            return 0
        else
            warn "AppImage skipped: linuxdeploy is not cached. Run '$0 github --branch $BRANCH' once while online, then local builds can use it offline."
            return 1
        fi
    fi
    printf '%s\n' "$tool"
}

ensure_flatpak_runtime() {
    local refs=(
        "org.gnome.Platform//$GNOME_RUNTIME"
        "org.gnome.Sdk//$GNOME_RUNTIME"
        "org.freedesktop.Sdk.Extension.rust-stable//$FDO_RUST_RUNTIME"
    )
    local missing=() ref
    for ref in "${refs[@]}"; do
        flatpak info "$ref" >/dev/null 2>&1 || missing+=("$ref")
    done

    ((${#missing[@]} == 0)) && return 0

    if ((ONLINE)); then
        log "Installing missing Flatpak build runtimes for the current user"
        flatpak remote-add --user --if-not-exists flathub \
            https://dl.flathub.org/repo/flathub.flatpakrepo
        flatpak install --user -y flathub "${missing[@]}"
    else
        printf '\nMissing Flatpak runtime/SDK required for OFFLINE mode:\n' >&2
        printf '  %s\n' "${missing[@]}" >&2
        printf '\nRun GitHub mode once while online to install/cache them, or install them manually.\n' >&2
        return 1
    fi
}

project_binary_name() {
    # Prefer the first explicit [[bin]] name. If Cargo.toml has no [[bin]],
    # Cargo uses the [package] name for src/main.rs.
    local explicit package
    explicit="$(awk '
        /^\[\[bin\]\]/ { in_bin=1; next }
        /^\[/ { if (in_bin) exit }
        in_bin && /^[[:space:]]*name[[:space:]]*=/ {
            line=$0; sub(/^[^"]*"/, "", line); sub(/".*$/, "", line); print line; exit
        }
    ' "$SOURCE_DIR/Cargo.toml")"
    if [[ -n "$explicit" ]]; then
        printf '%s\n' "$explicit"
        return
    fi
    package="$(awk '
        /^\[package\]/ { in_package=1; next }
        /^\[/ { if (in_package) exit }
        in_package && /^[[:space:]]*name[[:space:]]*=/ {
            line=$0; sub(/^[^"]*"/, "", line); sub(/".*$/, "", line); print line; exit
        }
    ' "$SOURCE_DIR/Cargo.toml")"
    [[ -n "$package" ]] || die "Could not determine the Cargo binary name. Set PIC_BIN_NAME manually."
    printf '%s\n' "$package"
}

project_version() {
    awk -F'"' '/^[[:space:]]*version[[:space:]]*=/ {print $2; exit}' "$SOURCE_DIR/Cargo.toml"
}

project_revision() {
    if [[ -d "$SOURCE_DIR/.git" ]] && have git; then
        git -C "$SOURCE_DIR" rev-parse --short=10 HEAD 2>/dev/null || true
    fi
}

normalize_png_icon() {
    local icon="$1" tmp
    tmp="${icon}.resize-tmp.png"

    if have magick; then
        magick "$icon" -resize '256x256' -background none -gravity center -extent '256x256' "$tmp"
    elif have convert; then
        convert "$icon" -resize '256x256' -background none -gravity center -extent '256x256' "$tmp"
    else
        warn "PNG application icon needs ImageMagick so it can be staged at 256x256. Install it with: sudo dnf install ImageMagick"
        return 1
    fi

    mv -f "$tmp" "$icon"
}

find_or_make_icon() {
    local out_dir="$1" candidate
    local candidates=(
        "$SOURCE_DIR/icon/pic-icon.png"
        "$SOURCE_DIR/icon/pic-icon.svg"
        "$SOURCE_DIR/icon/icon.png"
        "$SOURCE_DIR/icon/icon.svg"
    )
    for candidate in "${candidates[@]}"; do
        if [[ -f "$candidate" ]]; then
            ICON_EXT="${candidate##*.}"
            ICON_EXT="${ICON_EXT,,}"
            ICON_FILE="$out_dir/$APP_ID.$ICON_EXT"
            cp -f "$candidate" "$ICON_FILE"
            if [[ "$ICON_EXT" == png ]]; then
                normalize_png_icon "$ICON_FILE" || return 1
            fi
            return 0
        fi
    done

    candidate="$(find "$SOURCE_DIR" -maxdepth 3 -type f \( -iname '*.png' -o -iname '*.svg' \) \
        ! -path '*/target/*' ! -path '*/samples/*' | head -n 1 || true)"
    if [[ -n "$candidate" ]]; then
        ICON_EXT="${candidate##*.}"
        ICON_EXT="${ICON_EXT,,}"
        ICON_FILE="$out_dir/$APP_ID.$ICON_EXT"
        cp -f "$candidate" "$ICON_FILE"
        if [[ "$ICON_EXT" == png ]]; then
            normalize_png_icon "$ICON_FILE" || return 1
        fi
        warn "Expected icon/pic-icon.png was not found; using $candidate"
        return 0
    fi

    ICON_EXT=svg
    ICON_FILE="$out_dir/$APP_ID.svg"
    cat > "$ICON_FILE" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <rect width="256" height="256" rx="48" fill="#3584e4"/>
  <rect x="42" y="72" width="172" height="124" rx="22" fill="#fff"/>
  <circle cx="128" cy="134" r="42" fill="#3584e4"/>
  <circle cx="128" cy="134" r="24" fill="#fff"/>
  <path d="M82 72l18-24h56l18 24z" fill="#fff"/>
</svg>
SVG
    warn "No project icon found; generated a temporary PIC camera icon."
}

write_runtime_launcher() {
    local path="$1"
    cat > "$path" <<EOF_LAUNCHER
#!/bin/sh
set -eu

# AppImage launches this script through the top-level AppRun symlink. In that
# case \$0 points at AppRun, not usr/bin/$BIN_NAME, so derive the prefix from
# APPDIR (set by the AppImage runtime). Flatpak launches /app/bin/$BIN_NAME
# directly and does not set APPDIR, so keep the normal bin-directory fallback.
if [ -n "\${APPDIR:-}" ] && [ -d "\$APPDIR/usr/share/$BIN_NAME" ]; then
    PREFIX="\$APPDIR/usr"
else
    BIN_DIR="\$(CDPATH= cd -- "\$(dirname -- "\$0")" && pwd)"
    PREFIX="\$(dirname -- "\$BIN_DIR")"
fi

# Flatpak redirects XDG_DATA_HOME to \$HOME/.var/app/<app-id>/data by default,
# which would silently open a separate library. Keep the same library database
# and thumbnail cache as the native build so shares opened in either mode stay
# in sync.
if [ -n "\${FLATPAK_ID:-}" ]; then
    export XDG_DATA_HOME="\$HOME/.local/share"
    export XDG_CACHE_HOME="\$HOME/.cache"
fi

cd "\$PREFIX/share/$BIN_NAME"
exec "\$PREFIX/libexec/$BIN_NAME" "\$@"
EOF_LAUNCHER
    chmod +x "$path"
}

copy_runtime_resources() {
    local resource_root="$1"
    if [[ ! -d "$SOURCE_DIR/images" ]]; then
        warn "Runtime images folder not found: $SOURCE_DIR/images"
    else
        mkdir -p "$resource_root"
        rm -rf "$resource_root/images"
        cp -a "$SOURCE_DIR/images" "$resource_root/images"
        ok "Bundled runtime images: $resource_root/images"
    fi
    # Appearance themes: the app discovers themes/<name>/theme.css at
    # runtime, and users can drop extra theme folders in here.
    if [[ ! -d "$SOURCE_DIR/themes" ]]; then
        warn "Runtime themes folder not found: $SOURCE_DIR/themes"
    else
        rm -rf "$resource_root/themes"
        cp -a "$SOURCE_DIR/themes" "$resource_root/themes"
        ok "Bundled runtime themes: $resource_root/themes"
    fi
}

write_desktop_file() {
    local path="$1"
    cat > "$path" <<EOF_DESKTOP
[Desktop Entry]
Type=Application
Name=PIC - Picasa iPhoto Clone
Comment=Fast local photo manager inspired by Picasa and iPhoto
Exec=$BIN_NAME %F
Icon=$APP_ID
Terminal=false
StartupNotify=true
Categories=Graphics;Photography;
MimeType=image/jpeg;image/png;image/webp;image/gif;image/tiff;image/bmp;image/avif;image/heif;
EOF_DESKTOP
}

copy_source_tree() {
    local dest="$1"
    rm -rf "$dest"
    mkdir -p "$dest"
    (cd "$SOURCE_DIR" && tar \
        --exclude='./.git' \
        --exclude='./target' \
        --exclude='./dist' \
        --exclude='./.flatpak-builder' \
        -cf - .) | (cd "$dest" && tar -xf -)
}

build_native() {
    log "Testing/building Rust release binary OFFLINE"
    if [[ "$SKIP_TESTS" != 1 ]]; then
        if ! (cd "$SOURCE_DIR" && cargo test --locked --offline); then
            if [[ "$STRICT_TESTS" == 1 ]]; then
                die "cargo test failed and strict test mode is enabled."
            fi
            warn "cargo test failed. Packaging will continue; use --strict-tests if you want test failures to stop the build."
        fi
    else
        warn "Tests skipped (--skip-tests / PIC_SKIP_TESTS=1)."
    fi
    (cd "$SOURCE_DIR" && cargo build --release --locked --offline)
    NATIVE_BIN="$SOURCE_DIR/target/release/$BIN_NAME"
    [[ -x "$NATIVE_BIN" ]] || die "Release executable not found: $NATIVE_BIN"
    ok "Native release binary: $NATIVE_BIN"
}

build_appimage() {
    local linuxdeploy app_work appdir desktop staging_icon output_name deployed_bin real_bin resource_root
    if ! linuxdeploy="$(linuxdeploy_path)"; then
        return 1
    fi
    [[ -n "$linuxdeploy" && -x "$linuxdeploy" ]] || {
        warn "AppImage skipped: linuxdeploy is unavailable or not executable."
        return 1
    }

    app_work="$WORK_ROOT/appimage"
    appdir="$app_work/AppDir"
    rm -rf "$app_work"
    mkdir -p "$app_work" "$appdir"

    desktop="$app_work/$APP_ID.desktop"
    write_desktop_file "$desktop"
    find_or_make_icon "$app_work" || return 1
    staging_icon="$ICON_FILE"
    output_name="PIC-${BUILD_LABEL}-${ARCH_NAME}.AppImage"
    rm -f "$DIST_DIR/$output_name"

    log "Creating AppDir with linuxdeploy"
    APPIMAGE_EXTRACT_AND_RUN=1 "$linuxdeploy" \
        --appdir "$appdir" \
        --executable "$NATIVE_BIN" \
        --desktop-file "$desktop" \
        --icon-file "$staging_icon"

    # Keep the real executable separate and put a launcher at usr/bin/pic-rs.
    # The launcher changes into usr/share/pic-rs before starting PIC so existing
    # relative paths such as images/theme/... continue to work in the AppImage.
    deployed_bin="$appdir/usr/bin/$BIN_NAME"
    if [[ ! -f "$deployed_bin" ]]; then
        warn "AppImage staging did not contain the expected executable: $deployed_bin"
        return 1
    fi
    mkdir -p "$appdir/usr/libexec"
    real_bin="$appdir/usr/libexec/$BIN_NAME"
    mv -f "$deployed_bin" "$real_bin"
    chmod +x "$real_bin"
    write_runtime_launcher "$deployed_bin"

    resource_root="$appdir/usr/share/$BIN_NAME"
    copy_runtime_resources "$resource_root"

    # GTK4/libadwaita applications rely on GLib schemas and Adwaita symbolic icons.
    # linuxdeploy follows shared libraries; these data files are added explicitly.
    if [[ -d /usr/share/glib-2.0/schemas ]]; then
        mkdir -p "$appdir/usr/share/glib-2.0/schemas"
        cp -a /usr/share/glib-2.0/schemas/. "$appdir/usr/share/glib-2.0/schemas/"
        if have glib-compile-schemas; then
            glib-compile-schemas "$appdir/usr/share/glib-2.0/schemas" || true
        fi
    fi
    if [[ -d /usr/share/icons/Adwaita ]]; then
        mkdir -p "$appdir/usr/share/icons"
        cp -a /usr/share/icons/Adwaita "$appdir/usr/share/icons/"
    fi
    if [[ -f /usr/share/icons/hicolor/index.theme ]]; then
        mkdir -p "$appdir/usr/share/icons/hicolor"
        cp -f /usr/share/icons/hicolor/index.theme "$appdir/usr/share/icons/hicolor/"
    fi

    log "Writing AppImage: $DIST_DIR/$output_name"
    (
        cd "$DIST_DIR"
        ARCH="$APPIMAGE_ARCH" \
        LDAI_OUTPUT="$output_name" \
        LDAI_NO_APPSTREAM=1 \
        APPIMAGE_EXTRACT_AND_RUN=1 \
            "$linuxdeploy" --appdir "$appdir" --output appimage
    )
    [[ -s "$DIST_DIR/$output_name" ]] || return 1
    chmod +x "$DIST_DIR/$output_name"
    APPIMAGE_OUTPUT="$DIST_DIR/$output_name"
    ok "AppImage created: $APPIMAGE_OUTPUT"
}

build_flatpak() {
    local fp_work fp_src fp_build fp_repo manifest desktop_rel icon_rel bundle_name vendor_dir launcher_rel
    ensure_flatpak_runtime || return 1

    fp_work="$WORK_ROOT/flatpak"
    fp_src="$fp_work/flatpak-src"
    fp_build="$fp_work/build-dir"
    fp_repo="$fp_work/repo"
    manifest="$fp_work/$APP_ID.json"
    rm -rf "$fp_work"
    mkdir -p "$fp_work"
    copy_source_tree "$fp_src"

    mkdir -p "$fp_src/packaging-generated" "$fp_src/.cargo"
    write_desktop_file "$fp_src/packaging-generated/$APP_ID.desktop"
    write_runtime_launcher "$fp_src/packaging-generated/$BIN_NAME-launcher"
    find_or_make_icon "$fp_src/packaging-generated" || return 1
    icon_rel="packaging-generated/$APP_ID.$ICON_EXT"
    desktop_rel="packaging-generated/$APP_ID.desktop"
    launcher_rel="packaging-generated/$BIN_NAME-launcher"
    if [[ "$ICON_EXT" == svg ]]; then
        FLATPAK_ICON_DEST="/app/share/icons/hicolor/scalable/apps/$APP_ID.svg"
    else
        FLATPAK_ICON_DEST="/app/share/icons/hicolor/256x256/apps/$APP_ID.$ICON_EXT"
    fi

    log "Vendoring Rust crates for a network-free Flatpak build"
    vendor_dir="$fp_src/vendor"
    rm -rf "$vendor_dir"
    (cd "$fp_src" && cargo vendor --locked --offline vendor >/dev/null)
    cat > "$fp_src/.cargo/config.toml" <<'EOF_CARGO'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"

[net]
offline = true
EOF_CARGO

    cat > "$manifest" <<EOF_MANIFEST
{
  "app-id": "$APP_ID",
  "runtime": "org.gnome.Platform",
  "runtime-version": "$GNOME_RUNTIME",
  "sdk": "org.gnome.Sdk",
  "sdk-extensions": ["org.freedesktop.Sdk.Extension.rust-stable"],
  "command": "$BIN_NAME",
  "finish-args": [
    "--share=ipc",
    "--socket=wayland",
    "--socket=fallback-x11",
    "--device=dri",
    "--share=network",
    "--filesystem=host",
    "--filesystem=xdg-run/gvfsd",
    "--talk-name=org.gtk.vfs.*"
  ],
  "build-options": {
    "append-path": "/usr/lib/sdk/rust-stable/bin",
    "env": { "CARGO_NET_OFFLINE": "true" }
  },
  "modules": [
    {
      "name": "picasa-rs",
      "buildsystem": "simple",
      "build-commands": [
        "cargo build --release --locked --offline",
        "install -Dm755 target/release/$BIN_NAME /app/libexec/$BIN_NAME",
        "install -Dm755 $launcher_rel /app/bin/$BIN_NAME",
        "install -d /app/share/$BIN_NAME",
        "cp -a images /app/share/$BIN_NAME/",
        "cp -a themes /app/share/$BIN_NAME/",
        "install -Dm644 $desktop_rel /app/share/applications/$APP_ID.desktop",
        "install -Dm644 $icon_rel $FLATPAK_ICON_DEST"
      ],
      "sources": [
        { "type": "dir", "path": "flatpak-src" }
      ]
    }
  ]
}
EOF_MANIFEST

    log "Building Flatpak inside GNOME SDK (downloads disabled)"
    flatpak-builder \
        --force-clean \
        --disable-download \
        --repo="$fp_repo" \
        "$fp_build" "$manifest"

    bundle_name="PIC-${BUILD_LABEL}-${ARCH_NAME}.flatpak"
    rm -f "$DIST_DIR/$bundle_name"
    flatpak build-bundle "$fp_repo" "$DIST_DIR/$bundle_name" "$APP_ID"
    [[ -s "$DIST_DIR/$bundle_name" ]] || return 1
    FLATPAK_OUTPUT="$DIST_DIR/$bundle_name"
    ok "Flatpak bundle created: $FLATPAK_OUTPUT"
}

# -------------------- main --------------------
check_host_tools
if [[ "$MODE" == local ]]; then
    prepare_local_source
else
    prepare_github_source
fi

BIN_NAME="${BIN_NAME_OVERRIDE:-$(project_binary_name)}"
VERSION="$(project_version)"
VERSION="${VERSION:-0.0.0}"
REVISION="$(project_revision)"
BUILD_LABEL="$VERSION${REVISION:+-$REVISION}"

case "$(uname -m)" in
    x86_64|amd64) ARCH_NAME=x86_64; APPIMAGE_ARCH=x86_64 ;;
    aarch64|arm64) ARCH_NAME=aarch64; APPIMAGE_ARCH=aarch64 ;;
    i386|i486|i586|i686) ARCH_NAME=i686; APPIMAGE_ARCH=i686 ;;
    *) ARCH_NAME="$(uname -m)"; APPIMAGE_ARCH="$ARCH_NAME" ;;
esac

log "Build summary"
printf 'Mode:       %s\n' "$MODE"
printf 'Source:     %s\n' "$SOURCE_DIR"
printf 'Version:    %s\n' "$VERSION"
printf 'Revision:   %s\n' "${REVISION:-local-uncommitted}"
printf 'Binary:     %s\n' "$BIN_NAME"
printf 'App ID:     %s\n' "$APP_ID"
printf 'Output:     %s\n' "$DIST_DIR"
printf 'Build log:  %s\n' "$LOG_FILE"
printf 'Target:     %s\n' "$BUILD_TARGET"
if [[ "$BUILD_TARGET" == both || "$BUILD_TARGET" == flatpak ]]; then
    printf 'Flatpak:    GNOME %s + Rust extension %s\n' "$GNOME_RUNTIME" "$FDO_RUST_RUNTIME"
fi

build_native

appimage_ok=0
flatpak_ok=0
appimage_selected=0
flatpak_selected=0

if [[ "$BUILD_TARGET" == both || "$BUILD_TARGET" == appimage ]]; then
    appimage_selected=1
    if build_appimage; then
        appimage_ok=1
    else
        if [[ "$BUILD_TARGET" == both ]]; then
            warn "AppImage build failed; continuing so the Flatpak build still gets a chance."
        else
            warn "AppImage build failed."
        fi
    fi
fi

if [[ "$BUILD_TARGET" == both || "$BUILD_TARGET" == flatpak ]]; then
    flatpak_selected=1
    if build_flatpak; then
        flatpak_ok=1
    else
        warn "Flatpak build failed."
    fi
fi

printf '\n============================================================\n'
printf 'PIC Linux packaging finished\n'
printf '============================================================\n'
if ((appimage_selected)); then
    ((appimage_ok)) && printf 'AppImage: %s\n' "$APPIMAGE_OUTPUT" || printf 'AppImage: FAILED\n'
else
    printf 'AppImage: SKIPPED\n'
fi
if ((flatpak_selected)); then
    ((flatpak_ok)) && printf 'Flatpak:  %s\n' "$FLATPAK_OUTPUT" || printf 'Flatpak:  FAILED\n'
else
    printf 'Flatpak:  SKIPPED\n'
fi
printf 'Source:   %s (%s)\n' "$SOURCE_DIR" "$MODE"
printf 'Log:      %s\n' "$LOG_FILE"
printf '============================================================\n'

if ((appimage_selected && ! appimage_ok)); then
    exit 1
fi
if ((flatpak_selected && ! flatpak_ok)); then
    exit 1
fi

