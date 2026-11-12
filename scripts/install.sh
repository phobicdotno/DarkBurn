#!/bin/sh
# DarkBurn installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh | sh
#
# Status: pre-release. On Linux DarkBurn builds from source and its tests run. On macOS it has
# been built and tested on a build server only: nobody has used it on a Mac yet.
#
# What it does
#   1. Looks in the latest GitHub release for a ready-made build for this computer
#      (darkburn-<version>-<linux|macos>-<arch>.tar.gz). No release has one yet, so today this
#      step finds nothing and step 2 runs.
#   2. Builds from source: fetches DarkBurn and SolidCraft (its small UI crates are path
#      dependencies) side by side and runs `cargo build --release`. The first build takes ten
#      minutes or more and about 3.5 GB of disk.
#   3. Installs for the current user: the programs `darkburn` and `darkburn-mcp` in ~/.local/bin,
#      plus a menu entry and icon on Linux, or DarkBurn.app in ~/Applications on macOS.
#   It never uses sudo and installs nothing outside your home folder (a temporary folder is
#   used and removed). Run it again to update.
#
# Arguments (through the pipe: curl ... | sh -s -- --uninstall)
#   --help             Show the help text.
#   --uninstall        Remove what this installer put on the computer. Settings are kept.
#   --version <ref>    Install this branch, tag or commit instead of main. --ref is the same.
#
# Environment (through the pipe: curl ... | DARKBURN_FROM_SOURCE=1 sh)
#   DARKBURN_REPO            DarkBurn git URL or local path     [https://github.com/phobicdotno/DarkBurn]
#   DARKBURN_REF             Branch, tag or commit to install   [the latest release, else main]
#   SOLIDCRAFT_REPO          SolidCraft git URL or local path   [https://github.com/phobicdotno/solidcraft]
#   SOLIDCRAFT_REF           SolidCraft branch, tag or commit   [the commit DarkBurn is pinned to]
#   DARKBURN_PREFIX          Programs go into <prefix>/bin      [~/.local]
#   DARKBURN_BUILD_DIR       Where the source is kept and built [~/.cache/darkburn-build]
#   DARKBURN_FROM_SOURCE=1   Build from source even when a ready-made build exists
#   DARKBURN_INSTALL_RUST=1  Install Rust with rustup when it is missing or too old
#
# The whole script is one group in braces that closes on the last line. A shell reads such a
# group to its end before it runs any of it, so a download that was cut off runs nothing.
# The lines inside are not indented: the SOLIDCRAFT_PIN line below is read by other tools and
# has to start at the left edge.
{
set -eu

# The SolidCraft commit this version of DarkBurn builds against. Keep this line in this exact
# form: the installer reads it from the DarkBurn checkout it is about to build, so an older tag
# is built against the SolidCraft commit it was written for. install.ps1 has the same line.
SOLIDCRAFT_PIN='ae7da14907e00a11ed04656385cc0c2db386dc55'

GITHUB_REPO='phobicdotno/DarkBurn'
DEFAULT_DARKBURN_REPO='https://github.com/phobicdotno/DarkBurn'
DEFAULT_SOLIDCRAFT_REPO='https://github.com/phobicdotno/solidcraft'
INSTALL_URL='https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh'
# Used only when the checkout's Cargo.toml cannot be read.
MIN_RUST_FALLBACK='1.95'
# The window's application id (apps/darkburn/src/main.rs); the menu entry is named after it.
APP_ID='no.phobic.darkburn'
BUILD_MARKER='.darkburn-installer'
APP_MARKER='installed-by-darkburn-installer'

# ---------------------------------------------------------------------------------------------
# Messages
# ---------------------------------------------------------------------------------------------

say() { printf '%s\n' "$*"; }
step() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf '\nerror: %s\n' "$*" >&2
    exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat <<EOF
DarkBurn installer for Linux and macOS (pre-release)

Usage
  curl -fsSL $INSTALL_URL | sh
  curl -fsSL $INSTALL_URL | sh -s -- [--uninstall] [--version <ref>]
  sh install.sh [--help] [--uninstall] [--version <ref>]

Options
  --help             Show this text.
  --uninstall        Remove what this installer put on the computer. Settings are kept.
  --version <ref>    Install this branch, tag or commit instead of main. --ref is the same.

Environment
  DARKBURN_REPO            DarkBurn git URL or local path     [$DEFAULT_DARKBURN_REPO]
  DARKBURN_REF             Branch, tag or commit to install   [the latest release, else main]
  SOLIDCRAFT_REPO          SolidCraft git URL or local path   [$DEFAULT_SOLIDCRAFT_REPO]
  SOLIDCRAFT_REF           SolidCraft branch, tag or commit   [the commit DarkBurn is pinned to]
  DARKBURN_PREFIX          Programs go into <prefix>/bin      [~/.local]
  DARKBURN_BUILD_DIR       Where the source is kept and built [~/.cache/darkburn-build]
  DARKBURN_FROM_SOURCE=1   Build from source even when a ready-made build exists
  DARKBURN_INSTALL_RUST=1  Install Rust with rustup when it is missing or too old
  Through the pipe they go in front of sh:  curl ... | DARKBURN_INSTALL_RUST=1 sh

What it does
  1. Looks in the latest GitHub release for a ready-made build for this computer.
     No release has one yet, so today it goes straight to step 2.
  2. Builds from source with cargo. Needs git, Rust and a C compiler (on Linux also libclang).
     The first build takes ten minutes or more and about 3.5 GB of disk.
  3. Installs for the current user only. No sudo.
       Linux   <prefix>/bin/darkburn and darkburn-mcp, a menu entry and an icon
       macOS   ~/Applications/DarkBurn.app, with links in <prefix>/bin
  Run it again to update.

Status
  Pre-release. DarkBurn has not yet run a job on a real machine.
  On Linux it builds from source and its tests run. On macOS it has been built
  and tested on a build server only: nobody has used it on a Mac yet.
EOF
}

# ---------------------------------------------------------------------------------------------
# Small pure helpers (no side effects; exercised by a harness outside the repository)
# ---------------------------------------------------------------------------------------------

# True when dotted version $1 is at least $2. Missing fields count as 0 ("1.95" is "1.95.0").
version_ge() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        na = split(a, x, "."); nb = split(b, y, ".")
        n = (na > nb) ? na : nb
        for (i = 1; i <= n; i++) {
            p = (i <= na) ? x[i] + 0 : 0
            q = (i <= nb) ? y[i] + 0 : 0
            if (p > q) exit 0
            if (p < q) exit 1
        }
        exit 0
    }'
}

# True when $1 is a plain version number such as 0.0.509 (digits and dots, at least one dot).
is_version() {
    case "$1" in
        '' | .* | *. | *..* | *[!0-9.]*) return 1 ;;
        *.*) return 0 ;;
        *) return 1 ;;
    esac
}

# Prints "linux" or "macos" for a `uname -s` value, nothing for anything else.
os_of() {
    case "$1" in
        Linux) say linux ;;
        Darwin) say macos ;;
    esac
}

# Prints the architecture word used in release file names, for OS $1 and `uname -m` value $2.
# Nothing when there is no ready-made build for that processor.
arch_of() {
    case "$2" in
        x86_64 | amd64) say x86_64 ;;
        aarch64 | arm64)
            if [ "$1" = macos ]; then say arm64; else say aarch64; fi
            ;;
    esac
}

# The release file for version $1, OS $2 and architecture $3.
asset_name() { say "darkburn-$1-$2-$3.tar.gz"; }

# The JSON file $1 with every comma and brace turned into a line break, so that each
# "key": "value" pair of a GitHub API reply stands on a line of its own, pretty-printed or not.
json_lines() {
    awk '{ gsub(/[,{}]/, "\n"); print }' "$1"
}

# The first string value of key $1 in the JSON file $2. Good for the simple top-level fields of
# a GitHub API reply (tag_name); it does not understand nesting or escapes.
json_string() {
    json_lines "$2" |
        sed -n 's/^[[:space:]]*"'"$1"'"[[:space:]]*:[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' |
        head -n 1
}

# The download address of the release asset named $2 in the GitHub release reply $1, or nothing.
json_asset_url() {
    json_lines "$1" |
        sed -n 's/^[[:space:]]*"browser_download_url"[[:space:]]*:[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' |
        while IFS= read -r json_url; do
            case "$json_url" in
                https://*/"$2")
                    say "$json_url"
                    break
                    ;;
            esac
        done
}

# The value of `key = "value"` ($3) in section [$2] of the TOML file $1, or nothing.
toml_value() {
    awk -v section="[$2]" -v key="$3" '
        {
            line = $0
            sub(/\r$/, "", line)
            gsub(/^[ \t]+|[ \t]+$/, "", line)
        }
        substr(line, 1, 1) == "[" { inside = (line == section); next }
        inside {
            eq = index(line, "=")
            if (eq == 0) next
            k = substr(line, 1, eq - 1)
            gsub(/[ \t]+$/, "", k)
            if (k != key) next
            v = substr(line, eq + 1)
            if (match(v, /"[^"]*"/)) {
                print substr(v, RSTART + 1, RLENGTH - 2)
                exit
            }
        }
    ' "$1"
}

# The SolidCraft pin written in the installer file $1 (a DarkBurn checkout's copy), or nothing.
pin_from() {
    [ -f "$1" ] || return 0
    sed -n "s/^SOLIDCRAFT_PIN='\([0-9a-f]\{40\}\)'[[:space:]]*\$/\1/p" "$1" | head -n 1
}

# The version number in `rustc --version` output $1: "rustc 1.97.0-nightly (...)" gives 1.97.0.
rustc_version_of() {
    rv_word=$(printf '%s\n' "$1" | awk 'NR == 1 { print $2 }')
    say "${rv_word%%-*}"
}

# A path as one quoted argument of a desktop entry's Exec line (Desktop Entry Specification:
# backslash, quote, backtick and dollar are escaped, and the backslash doubled once more because
# the value is also a desktop-file string; a literal percent sign is written twice).
desktop_exec_quote() {
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\\\\\/g' -e 's/["`$]/\\\\&/g' -e 's/%/%%/g')"
}

# Lower-case hex SHA-256 of file $1, or nothing when no tool for it is installed.
sha256_of() {
    if has sha256sum; then
        sha256sum "$1" | awk '{ print $1 }'
    elif has shasum; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    elif has openssl; then
        openssl dgst -sha256 -r "$1" | awk '{ print $1 }'
    fi
}

# Makes $1 absolute (relative paths are taken from the current folder).
absolute_path() {
    case "$1" in
        /*) say "$1" ;;
        *) say "$PWD/$1" ;;
    esac
}

# A git source as given by the user: a local folder becomes an absolute path, a URL is kept.
repo_source() {
    if [ -d "$1" ]; then
        (cd "$1" && pwd)
    else
        say "$1"
    fi
}

# ---------------------------------------------------------------------------------------------
# Settings
# ---------------------------------------------------------------------------------------------

parse_args() {
    ACTION=install
    REF_ARG=
    while [ $# -gt 0 ]; do
        case "$1" in
            -h | --help) ACTION=help ;;
            --uninstall) ACTION=uninstall ;;
            --version | --ref)
                [ $# -ge 2 ] || die "$1 needs a branch, tag or commit after it."
                REF_ARG=$2
                shift
                ;;
            --version=* | --ref=*) REF_ARG=${1#*=} ;;
            *) die "unknown argument: $1 (try --help)" ;;
        esac
        shift
    done
}

settings() {
    [ -n "${HOME:-}" ] || die "HOME is not set, so there is nowhere to install to."

    OS=$(os_of "$(uname -s)")
    if [ -z "$OS" ]; then
        case "$(uname -s)" in
            MINGW* | MSYS* | CYGWIN*)
                die "this script is for Linux and macOS. On Windows run this in PowerShell:
    irm https://raw.githubusercontent.com/$GITHUB_REPO/main/scripts/install.ps1 | iex"
                ;;
            *) die "this script is for Linux and macOS, not $(uname -s)." ;;
        esac
    fi
    MACHINE=$(uname -m)
    # A terminal running under Rosetta reports x86_64 on an Apple silicon Mac.
    if [ "$OS" = macos ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
        MACHINE=arm64
    fi
    ARCH=$(arch_of "$OS" "$MACHINE")

    DARKBURN_URL=$(repo_source "${DARKBURN_REPO:-$DEFAULT_DARKBURN_REPO}")
    SOLIDCRAFT_URL=$(repo_source "${SOLIDCRAFT_REPO:-$DEFAULT_SOLIDCRAFT_REPO}")

    # What to install: --version/--ref wins over DARKBURN_REF. A bare version number means its tag.
    REF=${REF_ARG:-${DARKBURN_REF:-}}
    if is_version "$REF"; then REF="v$REF"; fi

    PREFIX=$(absolute_path "${DARKBURN_PREFIX:-$HOME/.local}")
    BIN_DIR="$PREFIX/bin"
    case "${XDG_CACHE_HOME:-}" in
        /*) cache_home=$XDG_CACHE_HOME ;;
        *) cache_home="$HOME/.cache" ;;
    esac
    BUILD_DIR=$(absolute_path "${DARKBURN_BUILD_DIR:-$cache_home/darkburn-build}")
    case "${XDG_DATA_HOME:-}" in
        /*) DATA_DIR=$XDG_DATA_HOME ;;
        *) DATA_DIR="$HOME/.local/share" ;;
    esac

    if [ "$OS" = macos ]; then
        APP_DIR="$HOME/Applications/DarkBurn.app"
        # Where DarkBurn keeps its settings (eframe's storage folder for the name "DarkBurn").
        SETTINGS_DIR="$HOME/Library/Application Support/DarkBurn"
    else
        DESKTOP_FILE="$DATA_DIR/applications/$APP_ID.desktop"
        ICON_DEST="$DATA_DIR/icons/hicolor/256x256/apps/darkburn.png"
        MIME_FILE="$DATA_DIR/mime/packages/$APP_ID.xml"
        SETTINGS_DIR="$DATA_DIR/darkburn"
    fi
}

# ---------------------------------------------------------------------------------------------
# Downloads
# ---------------------------------------------------------------------------------------------

# Fetches the https address $1 into the file $2. Quiet; fails on an HTTP error. Gives up when
# the server cannot be reached in 20 seconds, or sends nothing for 30 (wget) or 60 (curl).
download() {
    if has curl; then
        curl -fsSL --proto '=https' --tlsv1.2 --retry 2 --connect-timeout 20 \
            --speed-limit 1 --speed-time 60 -o "$2" "$1"
    elif has wget; then
        wget -q -T 30 -O "$2" "$1"
    else
        return 127
    fi
}

# Asks the GitHub API address $1 for a release and writes the reply into the file $2. Sets
# RELEASE_STATUS to the HTTP status of the reply (200, 404, ...), to 000 when there was no
# reply within a minute, and to "none" when neither curl nor wget is installed.
fetch_release() {
    RELEASE_STATUS=none
    if has curl; then
        RELEASE_STATUS=$(curl -sSL --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 60 \
            -o "$2" -w '%{http_code}' "$1" 2>/dev/null) || RELEASE_STATUS=000
    elif has wget; then
        # With -S wget prints the status line of every reply; the last one is the answer.
        RELEASE_STATUS=$(wget -q -S -T 30 -O "$2" "$1" 2>&1 |
            awk '$1 ~ /^HTTP\// { code = $2 } END { print code }') || RELEASE_STATUS=000
    fi
    case "$RELEASE_STATUS" in
        none | [1-5][0-9][0-9]) ;;
        *) RELEASE_STATUS=000 ;;
    esac
}

# ---------------------------------------------------------------------------------------------
# Step A: a ready-made build from a GitHub release
# ---------------------------------------------------------------------------------------------
#
# What a release file must look like for this step to use it:
#   name      darkburn-<version>-linux-x86_64.tar.gz, -linux-aarch64, -macos-x86_64 or -macos-arm64
#   contents  the programs `darkburn` and `darkburn-mcp`, at the top or inside one folder (a
#             `bin` folder is fine too); optionally `darkburn-256.png` beside them for the icon
#   checksum  optionally <name>.sha256 as a second asset: the SHA-256 in hex, first on its line
# Anything else is not an error: the installer builds from source instead.

# Sets PAYLOAD_DIR to the folder under $1 that holds both programs.
find_payload() {
    for payload_try in "$1" "$1"/* "$1"/bin "$1"/*/bin; do
        if [ -f "$payload_try/darkburn" ] && [ -f "$payload_try/darkburn-mcp" ]; then
            PAYLOAD_DIR=$payload_try
            return 0
        fi
    done
    return 1
}

# True when a release is what was asked for: not DARKBURN_FROM_SOURCE=1, not a repository of
# your own, and either no ref at all (the latest release) or a version tag such as v0.0.509.
# A branch or a commit has no release.
wants_prebuilt() {
    [ "${DARKBURN_FROM_SOURCE:-}" != 1 ] || return 1
    [ -z "${DARKBURN_REPO:-}" ] || return 1
    [ -z "$REF" ] || is_version "${REF#v}"
}

# Runs "$@" with nothing on its input and its output thrown away. True when it ends by itself,
# without an error, within 30 seconds; a program that is still running then is stopped.
starts_here() {
    "$@" </dev/null >/dev/null 2>&1 &
    start_pid=$!
    (
        sleep 30
        kill "$start_pid" 2>/dev/null
    ) >/dev/null 2>&1 &
    start_watch=$!
    if wait "$start_pid" 2>/dev/null; then start_status=0; else start_status=$?; fi
    kill "$start_watch" 2>/dev/null || true
    wait "$start_watch" 2>/dev/null || true
    return "$start_status"
}

# Returns 0 with PAYLOAD_DIR, ICON_FILE and VERSION set when a ready-made build was fetched and
# runs here. Returns 1 when there is none, with the reason in PREBUILT_WHY (empty when no
# release was asked for, or when a warning has already said why). Stops only on a wrong checksum.
try_prebuilt() {
    PREBUILT_WHY=
    wants_prebuilt || return 1
    if [ -z "$ARCH" ]; then
        PREBUILT_WHY="There are no ready-made builds for $MACHINE processors."
        return 1
    fi
    if ! has tar; then
        PREBUILT_WHY='tar is not installed, so a ready-made build could not be unpacked.'
        return 1
    fi
    if [ -z "$REF" ]; then
        release_api="https://api.github.com/repos/$GITHUB_REPO/releases/latest"
    else
        release_api="https://api.github.com/repos/$GITHUB_REPO/releases/tags/$REF"
    fi

    step 'Looking for a ready-made build in the GitHub releases'
    fetch_release "$release_api" "$TMP_DIR/release.json"
    case "$RELEASE_STATUS" in
        200) ;;
        404)
            if [ -z "$REF" ]; then
                PREBUILT_WHY='No release has been published yet.'
            else
                PREBUILT_WHY="There is no release $REF."
            fi
            return 1
            ;;
        000)
            PREBUILT_WHY='The GitHub release list did not answer.'
            return 1
            ;;
        none)
            PREBUILT_WHY='Neither curl nor wget is installed, so the release list was not read.'
            return 1
            ;;
        *)
            PREBUILT_WHY="GitHub answered the release lookup with HTTP $RELEASE_STATUS, so it could not be read."
            return 1
            ;;
    esac
    release_tag=$(json_string tag_name "$TMP_DIR/release.json")
    if ! is_version "${release_tag#v}"; then
        PREBUILT_WHY='The reply from the GitHub release list could not be read.'
        return 1
    fi
    VERSION=${release_tag#v}
    release_asset=$(asset_name "$VERSION" "$OS" "$ARCH")
    release_url=$(json_asset_url "$TMP_DIR/release.json" "$release_asset")
    if [ -z "$release_url" ]; then
        PREBUILT_WHY="Release $release_tag has no ready-made build for $OS $MACHINE."
        return 1
    fi

    step "Downloading $release_asset"
    if ! download "$release_url" "$TMP_DIR/$release_asset"; then
        warn "the download did not finish; building from source instead."
        return 1
    fi

    release_sum_url=$(json_asset_url "$TMP_DIR/release.json" "$release_asset.sha256")
    if [ -n "$release_sum_url" ]; then
        if ! download "$release_sum_url" "$TMP_DIR/$release_asset.sha256"; then
            warn "the checksum file did not download; building from source instead."
            return 1
        fi
        sum_want=$(awk 'NR == 1 { print $1 }' "$TMP_DIR/$release_asset.sha256" | tr 'A-F' 'a-f')
        sum_have=$(sha256_of "$TMP_DIR/$release_asset" | tr 'A-F' 'a-f')
        if [ -z "$sum_have" ]; then
            warn "no sha256sum, shasum or openssl here to check the download; building from source instead."
            return 1
        fi
        if [ "$sum_want" != "$sum_have" ]; then
            die "the SHA-256 of $release_asset does not match the one published with the release.
  published  $sum_want
  downloaded $sum_have
Nothing was installed. Try again; if it keeps happening, please report it at
https://github.com/$GITHUB_REPO/issues. DARKBURN_FROM_SOURCE=1 builds from source instead."
        fi
        say "    checksum ok"
    fi

    mkdir -p "$TMP_DIR/unpack" || return 1
    if ! tar -xzf "$TMP_DIR/$release_asset" -C "$TMP_DIR/unpack" 2>/dev/null; then
        warn "$release_asset could not be unpacked; building from source instead."
        return 1
    fi
    if ! find_payload "$TMP_DIR/unpack"; then
        warn "$release_asset does not hold darkburn and darkburn-mcp; building from source instead."
        return 1
    fi
    chmod 755 "$PAYLOAD_DIR/darkburn" "$PAYLOAD_DIR/darkburn-mcp" 2>/dev/null || true
    # A build for another C library or a newer system does not start: find out before installing.
    # Each program is started once, as the two do not need the same things from the system.
    # With nothing on its input, darkburn --mcp-headless starts without a window and stops.
    if ! starts_here "$PAYLOAD_DIR/darkburn-mcp" --version || ! starts_here "$PAYLOAD_DIR/darkburn" --mcp-headless; then
        warn "the ready-made build does not run on this system; building from source instead."
        return 1
    fi

    ICON_FILE=
    if [ -f "$PAYLOAD_DIR/darkburn-256.png" ]; then
        ICON_FILE="$PAYLOAD_DIR/darkburn-256.png"
    elif download "https://raw.githubusercontent.com/$GITHUB_REPO/$release_tag/assets/icons/app/darkburn-256.png" \
        "$TMP_DIR/darkburn-256.png" 2>/dev/null; then
        ICON_FILE="$TMP_DIR/darkburn-256.png"
    fi
    SOURCE_NOTE="release $release_tag"
    return 0
}

# ---------------------------------------------------------------------------------------------
# Step B: build from source
# ---------------------------------------------------------------------------------------------

add_cargo_to_path() {
    cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
    if [ -x "$cargo_bin/cargo" ]; then
        case ":$PATH:" in
            *":$cargo_bin:"*) ;;
            *) PATH="$cargo_bin:$PATH" ;;
        esac
    fi
}

# libclang is needed while building (the camera crate generates its Linux bindings with it).
# Looks where the build looks: LIBCLANG_PATH, llvm-config, the library path, the usual folders.
have_libclang() {
    [ -z "${LIBCLANG_PATH:-}" ] || return 0
    clang_prefix=
    if has llvm-config; then clang_prefix=$(llvm-config --prefix 2>/dev/null || true); fi
    clang_ifs=$IFS
    IFS=:
    # The library path is split on colons here and nowhere else.
    # shellcheck disable=SC2086
    set -- ${LD_LIBRARY_PATH:-} ${LIBRARY_PATH:-}
    IFS=$clang_ifs
    for clang_dir in "$@" ${clang_prefix:+"$clang_prefix/lib"} ${clang_prefix:+"$clang_prefix/lib64"} \
        /usr/lib* /usr/lib*/* /usr/lib*/*/* /usr/local/lib* /usr/local/lib*/* /usr/local/lib*/*/* \
        /usr/local/llvm*/lib*; do
        [ -d "$clang_dir" ] || continue
        for clang_lib in "$clang_dir"/libclang.so "$clang_dir"/libclang.so.* \
            "$clang_dir"/libclang-*.so "$clang_dir"/libclang-*.so.*; do
            case "$clang_lib" in
                *-cpp.*) continue ;;
            esac
            [ -e "$clang_lib" ] && return 0
        done
    done
    return 1
}

# The one line that installs the build tools on this kind of Linux.
linux_packages_hint() {
    if has apt-get; then
        say "    sudo apt install git curl build-essential libclang-dev"
    elif has dnf; then
        say "    sudo dnf install git curl gcc clang-devel"
    elif has pacman; then
        say "    sudo pacman -S --needed git curl base-devel clang"
    elif has zypper; then
        say "    sudo zypper install git curl gcc clang-devel"
    elif has apk; then
        say "    apk add git curl build-base clang-dev linux-headers"
    else
        say "    git, a C compiler with the C library headers (gcc), and libclang,"
        say "    from your distribution's packages"
    fi
}

install_rust() {
    step "Installing Rust with rustup (stable toolchain, for this user only)"
    download https://sh.rustup.rs "$TMP_DIR/rustup-init.sh" ||
        die "could not download the Rust installer from https://sh.rustup.rs. Check the network and try again."
    sh "$TMP_DIR/rustup-init.sh" -y --profile minimal --default-toolchain stable ||
        die "rustup did not finish. Install Rust by hand from https://rustup.rs and run this installer again."
    add_cargo_to_path
}

# Stops with a list of what to install when a build tool is missing.
check_build_tools() {
    tools_missing=
    has git || tools_missing="$tools_missing
    git"
    if [ "$OS" = linux ]; then
        has cc || tools_missing="$tools_missing
    a C compiler (cc)"
        have_libclang || tools_missing="$tools_missing
    libclang"
        if [ -n "$tools_missing" ]; then
            die "the build needs these, and they were not found:$tools_missing
Install the build tools with:
$(linux_packages_hint)
and run this installer again. It does not install system packages itself.
(libclang in an unusual place? Set LIBCLANG_PATH to the folder that holds libclang.so.)"
        fi
    else
        if ! xcode-select -p >/dev/null 2>&1 || ! has cc || [ -n "$tools_missing" ]; then
            die "the Xcode command line tools (compiler, linker and git) are missing.
Install them with:
    xcode-select --install
and run this installer again."
        fi
    fi

    add_cargo_to_path
    if ! has cargo || ! has rustc; then
        if [ "${DARKBURN_INSTALL_RUST:-}" = 1 ]; then
            install_rust
            if ! has cargo || ! has rustc; then
                die "Rust was installed but cargo is still not found. Open a new terminal and run this installer again."
            fi
        else
            die "Rust is needed to build DarkBurn and was not found.
Install it with:
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
then open a new terminal and run this installer again.
Or let this installer do it:
    curl -fsSL $INSTALL_URL | DARKBURN_INSTALL_RUST=1 sh"
        fi
    fi
}

# Stops when rustc is older than the rust-version of the checkout $1. With DARKBURN_INSTALL_RUST=1
# it gets a newer one first: from rustup when the old one is rustup's, else by installing rustup.
check_rust_version() {
    rust_need=$(toml_value "$1/Cargo.toml" workspace.package rust-version)
    [ -n "$rust_need" ] || rust_need=$MIN_RUST_FALLBACK
    rust_have=$(rustc_version_of "$(cd "$1" && rustc --version 2>/dev/null)")
    if [ -n "$rust_have" ] && version_ge "$rust_have" "$rust_need"; then
        say "    Rust $rust_have (DarkBurn needs $rust_need or newer)"
        return 0
    fi
    if [ -z "$rust_have" ] && [ "${DARKBURN_INSTALL_RUST:-}" != 1 ]; then
        die "rustc is installed but does not run:
$(cd "$1" && rustc --version 2>&1 | head -n 3 | sed 's/^/    /')
With rustup, pick a toolchain with:
    rustup default stable
and run this installer again."
    fi
    if [ "${DARKBURN_INSTALL_RUST:-}" = 1 ]; then
        if has rustup; then
            step "Rust ${rust_have:-?} is older than $rust_need: installing the stable toolchain with rustup"
            rustup toolchain install stable --profile minimal ||
                die "rustup could not install the stable toolchain."
            RUSTUP_TOOLCHAIN=stable
            export RUSTUP_TOOLCHAIN
        else
            # This Rust is not rustup's (a distribution package, say), so rustup cannot update it.
            # rustup puts the current Rust in its own folder, and that folder goes first on PATH
            # for the rest of this run. The old Rust is left as it is.
            step "Rust ${rust_have:-?} is older than $rust_need and does not come from rustup: installing rustup beside it"
            install_rust
            PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
        fi
        rust_have=$(rustc_version_of "$(cd "$1" && rustc --version 2>/dev/null)")
        if [ -n "$rust_have" ] && version_ge "$rust_have" "$rust_need"; then
            say "    Rust $rust_have (DarkBurn needs $rust_need or newer)"
            return 0
        fi
        die "Rust ${rust_have:-?} is still too old after the install with rustup: DarkBurn needs $rust_need or newer.
The rustc in use is $(command -v rustc 2>/dev/null || say 'not found').
If that is not rustup's, put ${CARGO_HOME:-$HOME/.cargo}/bin first on your PATH and run this installer again."
    fi
    if has rustup; then
        die "Rust ${rust_have:-?} is too old: DarkBurn needs $rust_need or newer.
Update it with:
    rustup update stable
and run this installer again (or set DARKBURN_INSTALL_RUST=1 and it does that for you)."
    fi
    die "Rust ${rust_have:-?} is too old: DarkBurn needs $rust_need or newer.
This Rust does not come from rustup. Install the current one with:
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
then open a new terminal and run this installer again.
Or let this installer do it (the old Rust stays, rustup goes in beside it):
    curl -fsSL $INSTALL_URL | DARKBURN_INSTALL_RUST=1 sh"
}

# The build folder is reused between runs. It is marked so that neither a re-run nor --uninstall
# ever touches a folder this installer did not make.
prepare_build_dir() {
    if [ -e "$BUILD_DIR" ] && [ ! -f "$BUILD_DIR/$BUILD_MARKER" ]; then
        if [ ! -d "$BUILD_DIR" ] || [ -n "$(ls -A "$BUILD_DIR" 2>/dev/null)" ]; then
            die "$BUILD_DIR exists and was not made by this installer.
Set DARKBURN_BUILD_DIR to an empty or new folder."
        fi
    fi
    mkdir -p "$BUILD_DIR" || die "could not make $BUILD_DIR."
    say "Made by the DarkBurn installer (scripts/install.sh). Safe to delete; --uninstall removes it." \
        >"$BUILD_DIR/$BUILD_MARKER" || die "could not write to $BUILD_DIR."
}

# Brings the checkout in folder $2 to ref $4 of repository $3 ($1 is the name for messages).
# Shallow when the server allows it (GitHub does, also for a commit id), full history otherwise.
#
# Every git call names the checkout outright (--git-dir and --work-tree). With `git -C` git
# would look for a repository by itself, and when the checkout is not one (an init that was cut
# off) it would find the one around it, for example a home folder kept in git, and check
# DarkBurn out over that.
sync_repo() {
    # A checkout that is not a repository is made again. It is inside the marked build folder.
    if [ -e "$2" ] && ! git --git-dir="$2/.git" rev-parse --git-dir >/dev/null 2>&1; then
        rm -rf "$2" || die "could not remove $2."
    fi
    if [ ! -d "$2/.git" ]; then
        mkdir -p "$2" || die "could not make $2."
        git --git-dir="$2/.git" --work-tree="$2" init -q || die "git init failed in $2."
    fi
    step "Fetching $1 ($4)"
    if GIT_TERMINAL_PROMPT=0 git --git-dir="$2/.git" --work-tree="$2" fetch --depth 1 "$3" "$4"; then
        git --git-dir="$2/.git" --work-tree="$2" checkout -q -f --detach FETCH_HEAD ||
            die "could not check out $1 $4.
If it keeps happening, delete $2 and run the installer again."
        return 0
    fi
    # No shallow fetch of that ref (a short commit id, or a server that refuses): take it all.
    # The depth is git's "everything", which also completes a checkout that was shallow before.
    say "    $4 cannot be fetched on its own. Fetching the full history."
    if ! GIT_TERMINAL_PROMPT=0 git --git-dir="$2/.git" --work-tree="$2" fetch --depth 2147483647 --tags "$3" '+refs/heads/*:refs/remotes/origin/*'; then
        die "could not fetch $1 from $3.
Check the network. If the repository is private or has moved, point ${5:-the repository setting} at a copy you can reach.
If the lines above name a lock file or a broken repository, delete $2 and run the installer again."
    fi
    for sync_try in "refs/remotes/origin/$4" "refs/tags/$4" "$4"; do
        if sync_commit=$(git --git-dir="$2/.git" --work-tree="$2" rev-parse -q --verify "$sync_try^{commit}" 2>/dev/null); then
            git --git-dir="$2/.git" --work-tree="$2" checkout -q -f --detach "$sync_commit" ||
                die "could not check out $1 $4.
If it keeps happening, delete $2 and run the installer again."
            return 0
        fi
    done
    die "$1 has no branch, tag or commit named $4."
}

# Warns when the build folder's disk is nearly full. A release build needs about 3.5 GB.
check_disk_space() {
    disk_free_kb=$(df -Pk "$BUILD_DIR" 2>/dev/null | awk 'NR == 2 { print $4 }')
    case "$disk_free_kb" in
        '' | *[!0-9]*) return 0 ;;
    esac
    if [ "$disk_free_kb" -lt 4194304 ]; then
        warn "only $((disk_free_kb / 1024)) MB free in $BUILD_DIR. The build needs about 3.5 GB and may run out of space."
    fi
}

build_from_source() {
    check_build_tools
    prepare_build_dir
    # Git settings of the calling shell must not point the git calls at another repository.
    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES \
        GIT_COMMON_DIR GIT_NAMESPACE

    SOURCE_DIR="$BUILD_DIR/DarkBurn"
    sync_repo DarkBurn "$SOURCE_DIR" "$DARKBURN_URL" "${REF:-main}" DARKBURN_REPO
    [ -f "$SOURCE_DIR/Cargo.toml" ] || die "$DARKBURN_URL at ${REF:-main} is not a DarkBurn checkout (no Cargo.toml)."
    check_rust_version "$SOURCE_DIR"

    # DarkBurn's Cargo.toml reaches for ../solidcraft, so the two checkouts sit side by side.
    solidcraft_ref=${SOLIDCRAFT_REF:-}
    [ -n "$solidcraft_ref" ] || solidcraft_ref=$(pin_from "$SOURCE_DIR/scripts/install.sh")
    [ -n "$solidcraft_ref" ] || solidcraft_ref=$SOLIDCRAFT_PIN
    sync_repo SolidCraft "$BUILD_DIR/solidcraft" "$SOLIDCRAFT_URL" "$solidcraft_ref" SOLIDCRAFT_REPO

    check_disk_space
    step "Building DarkBurn (cargo build --release). The first build takes ten minutes or more."
    # One fixed place for the output, whatever CARGO_TARGET_DIR says elsewhere.
    if ! (cd "$SOURCE_DIR" && CARGO_TARGET_DIR="$BUILD_DIR/target" \
        cargo build --release --locked -p darkburn -p darkburn-mcp); then
        die "the build failed; the lines above say why.
Nothing was installed or changed. The source and what was built so far stay in
  $BUILD_DIR
so running the installer again continues from there."
    fi

    PAYLOAD_DIR=
    for build_try in "$BUILD_DIR/target/release" "$BUILD_DIR"/target/*/release; do
        if [ -f "$build_try/darkburn" ] && [ -f "$build_try/darkburn-mcp" ]; then
            PAYLOAD_DIR=$build_try
            break
        fi
    done
    [ -n "$PAYLOAD_DIR" ] || die "the build finished but darkburn and darkburn-mcp are not in $BUILD_DIR/target."

    VERSION=$(toml_value "$SOURCE_DIR/Cargo.toml" workspace.package version)
    ICON_FILE=
    [ ! -f "$SOURCE_DIR/assets/icons/app/darkburn-256.png" ] || ICON_FILE="$SOURCE_DIR/assets/icons/app/darkburn-256.png"
    SOURCE_NOTE="built from ${REF:-main} at $(git --git-dir="$SOURCE_DIR/.git" rev-parse --short HEAD 2>/dev/null || say '?')"
}

# ---------------------------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------------------------

# Puts program $1 at $2. The copy is renamed into place, so a running DarkBurn keeps working
# until it is restarted.
install_program() {
    if ! cp "$1" "$2.new.$$" || ! chmod 755 "$2.new.$$" || ! mv -f "$2.new.$$" "$2"; then
        rm -f "$2.new.$$"
        die "could not write $2."
    fi
}

install_linux() {
    mkdir -p "$BIN_DIR" || die "could not make $BIN_DIR."
    install_program "$PAYLOAD_DIR/darkburn" "$BIN_DIR/darkburn"
    install_program "$PAYLOAD_DIR/darkburn-mcp" "$BIN_DIR/darkburn-mcp"

    mkdir -p "$(dirname "$DESKTOP_FILE")" "$(dirname "$ICON_DEST")" "$(dirname "$MIME_FILE")" ||
        die "could not make the folders under $DATA_DIR."
    if [ -n "$ICON_FILE" ]; then
        cp "$ICON_FILE" "$ICON_DEST" || die "could not write $ICON_DEST."
    fi
    # Projects (.dbrn) open with DarkBurn, as the Windows installer registers them.
    cat >"$MIME_FILE" <<EOF || die "could not write $MIME_FILE."
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-darkburn-project">
    <comment>DarkBurn project</comment>
    <glob pattern="*.dbrn"/>
  </mime-type>
</mime-info>
EOF
    cat >"$DESKTOP_FILE" <<EOF || die "could not write $DESKTOP_FILE."
[Desktop Entry]
Type=Application
Name=DarkBurn
GenericName=Laser cutter and engraver
Comment=Design, arrange and burn with a laser cutter or engraver
Exec=$(desktop_exec_quote "$BIN_DIR/darkburn") %F
Icon=darkburn
Terminal=false
Categories=Graphics;VectorGraphics;2DGraphics;
MimeType=application/x-darkburn-project;
StartupWMClass=darkburn
EOF
    refresh_desktop_caches
}

# The desktop's lookup tables, where the tools for them are installed. Never fatal.
refresh_desktop_caches() {
    if has update-mime-database; then update-mime-database "$DATA_DIR/mime" >/dev/null 2>&1 || true; fi
    if has update-desktop-database; then update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true; fi
    # An icon cache is refreshed when there is one. None is made: a cache nobody keeps up to date
    # hides icons that other programs install later, and without one the folders are read as they are.
    if has gtk-update-icon-cache && [ -f "$DATA_DIR/icons/hicolor/icon-theme.cache" ]; then
        gtk-update-icon-cache -q -t -f "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

# DarkBurn.icns from the 256 px picture $1 with the tools macOS ships. Returns 1 without them.
make_icns() {
    if ! has sips || ! has iconutil; then return 1; fi
    icns_set="$TMP_DIR/DarkBurn.iconset"
    rm -rf "$icns_set"
    mkdir -p "$icns_set" || return 1
    for icns_spec in 16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x \
        128:icon_128x128 256:icon_128x128@2x 256:icon_256x256; do
        sips -z "${icns_spec%%:*}" "${icns_spec%%:*}" "$1" --out "$icns_set/${icns_spec#*:}.png" >/dev/null 2>&1 || return 1
    done
    iconutil -c icns -o "$2" "$icns_set" >/dev/null 2>&1 || return 1
}

# Stops when there is a DarkBurn.app that this installer did not make. Asked before the build,
# so nobody waits for a build that cannot be installed, and again before the app is written.
check_app_dir() {
    if [ -e "$APP_DIR" ] && [ ! -f "$APP_DIR/Contents/$APP_MARKER" ]; then
        die "$APP_DIR exists and was not made by this installer.
Move it away and run the installer again."
    fi
}

install_macos() {
    check_app_dir
    mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources" "$BIN_DIR" ||
        die "could not make $APP_DIR."
    say "Made by the DarkBurn installer (scripts/install.sh)." >"$APP_DIR/Contents/$APP_MARKER" ||
        die "could not write to $APP_DIR."
    install_program "$PAYLOAD_DIR/darkburn" "$APP_DIR/Contents/MacOS/darkburn"
    install_program "$PAYLOAD_DIR/darkburn-mcp" "$APP_DIR/Contents/MacOS/darkburn-mcp"

    plist_icon=
    if [ -n "$ICON_FILE" ] && make_icns "$ICON_FILE" "$APP_DIR/Contents/Resources/DarkBurn.icns"; then
        plist_icon='    <key>CFBundleIconFile</key>
    <string>DarkBurn</string>'
    fi
    cat >"$APP_DIR/Contents/Info.plist" <<EOF || die "could not write $APP_DIR/Contents/Info.plist."
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>DarkBurn</string>
    <key>CFBundleDisplayName</key>
    <string>DarkBurn</string>
    <key>CFBundleIdentifier</key>
    <string>$APP_ID</string>
    <key>CFBundleExecutable</key>
    <string>darkburn</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION:-0}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION:-0}</string>
$plist_icon
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSCameraUsageDescription</key>
    <string>DarkBurn shows the camera picture only after you turn the camera on and pick one.</string>
</dict>
</plist>
EOF
    # Tell Finder the bundle changed.
    touch "$APP_DIR" 2>/dev/null || true

    ln -sf "$APP_DIR/Contents/MacOS/darkburn" "$BIN_DIR/darkburn" || die "could not link $BIN_DIR/darkburn."
    ln -sf "$APP_DIR/Contents/MacOS/darkburn-mcp" "$BIN_DIR/darkburn-mcp" || die "could not link $BIN_DIR/darkburn-mcp."
}

# True when libxkbcommon-x11 is installed. DarkBurn loads it when it starts in an X11 session
# and stops without it; `ldd` does not show it, because it is loaded while the program runs.
have_xkb_x11() {
    for xkb_tool in ldconfig /sbin/ldconfig /usr/sbin/ldconfig; do
        xkb_list=$("$xkb_tool" -p 2>/dev/null) || continue
        case "$xkb_list" in
            *libxkbcommon-x11.so*) return 0 ;;
        esac
    done
    # Not in the library cache, or no cache to ask (musl has none): look in the usual folders.
    for xkb_lib in /usr/lib*/libxkbcommon-x11.so* /usr/lib*/*/libxkbcommon-x11.so* \
        /lib*/libxkbcommon-x11.so* /lib*/*/libxkbcommon-x11.so* /usr/local/lib*/libxkbcommon-x11.so*; do
        [ -e "$xkb_lib" ] && return 0
    done
    return 1
}

# The package that holds libxkbcommon-x11 on this kind of Linux.
xkb_x11_hint() {
    if has apt-get; then
        say "    sudo apt install libxkbcommon-x11-0"
    elif has dnf; then
        say "    sudo dnf install libxkbcommon-x11"
    elif has pacman; then
        say "    sudo pacman -S --needed libxkbcommon-x11"
    elif has zypper; then
        say "    sudo zypper install libxkbcommon-x11-0"
    elif has apk; then
        say "    apk add libxkbcommon-x11"
    else
        say "    libxkbcommon-x11, from your distribution's packages"
    fi
}

# The file a new terminal reads, for the PATH line. A name to show, so the tilde stays as it is.
profile_name() {
    # shellcheck disable=SC2088
    case "${SHELL:-}" in
        */zsh) if [ "$OS" = macos ]; then say '~/.zprofile'; else say '~/.zshrc'; fi ;;
        */bash) if [ "$OS" = macos ]; then say '~/.bash_profile'; else say '~/.profile'; fi ;;
        *) say '~/.profile' ;;
    esac
}

finish() {
    installed_version=$("$BIN_DIR/darkburn-mcp" --version 2>/dev/null | awk 'NR == 1 { print $2 }' || true)
    say ""
    if [ -n "$installed_version" ]; then
        say "DarkBurn $installed_version is installed ($SOURCE_NOTE)."
    else
        say "DarkBurn ${VERSION:-} is installed ($SOURCE_NOTE)."
        warn "$BIN_DIR/darkburn-mcp --version did not answer; the programs may not run on this system."
    fi
    say "  program     $BIN_DIR/darkburn"
    say "  MCP server  $BIN_DIR/darkburn-mcp"
    if [ "$OS" = macos ]; then
        say "  app         $APP_DIR"
    else
        say "  menu entry  $DESKTOP_FILE"
    fi
    say "  settings    $SETTINGS_DIR"
    say ""
    if [ "$OS" = macos ]; then
        say "Start it from ~/Applications (Finder or Spotlight), or run: darkburn"
        say "On macOS DarkBurn has been built and tested on a build server only: nobody has used it, or this installer, on a Mac yet."
        say "Please report what happens: https://github.com/$GITHUB_REPO/issues"
    else
        say "Start it from the applications menu, or run: darkburn"
    fi
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            say ""
            say "$BIN_DIR is not on your PATH. Until it is, start DarkBurn with the full path"
            say "$BIN_DIR/darkburn, or add this line to $(profile_name) (or the file your shell reads):"
            say "    export PATH=\"$BIN_DIR:\$PATH\""
            ;;
    esac
    if [ "$OS" = linux ] && ! have_xkb_x11; then
        say ""
        warn "libxkbcommon-x11 was not found. In an X11 session DarkBurn stops at start without it
(under Wayland it is not needed). Install it with:
$(xkb_x11_hint)"
    fi
    if [ -n "$BUILT_HERE" ]; then
        say ""
        say "The source and the build stay in $BUILD_DIR"
        say "so the next update is quick. Delete that folder to get the disk space back."
    fi
    say ""
    say "Pre-release: DarkBurn has not yet run a job on a real machine."
    say "Update: run this installer again.  Remove: curl -fsSL $INSTALL_URL | sh -s -- --uninstall"
}

# ---------------------------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------------------------

remove_file() {
    if [ -f "$1" ] || [ -L "$1" ]; then
        rm -f "$1" || die "could not remove $1."
        say "  removed $1"
        REMOVED=1
    fi
}

# Only for folders this installer makes: DarkBurn.app (marked), and the three it makes inside
# the marked build folder.
remove_folder() {
    if [ -d "$1" ]; then
        rm -rf "$1" || die "could not remove $1."
        say "  removed $1"
        REMOVED=1
    fi
}

# The build folder: the two checkouts, the build output and the marker go. The folder itself
# goes only when nothing else is in it, so files of your own that ended up there are kept.
remove_build_dir() {
    [ -e "$BUILD_DIR" ] || return 0
    if [ ! -f "$BUILD_DIR/$BUILD_MARKER" ]; then
        say "  kept $BUILD_DIR (it was not made by this installer)"
        return 0
    fi
    remove_folder "$BUILD_DIR/DarkBurn"
    remove_folder "$BUILD_DIR/solidcraft"
    remove_folder "$BUILD_DIR/target"
    rm -f "$BUILD_DIR/$BUILD_MARKER" || die "could not remove $BUILD_DIR/$BUILD_MARKER."
    if rmdir "$BUILD_DIR" 2>/dev/null; then
        say "  removed $BUILD_DIR"
        REMOVED=1
    else
        say "  kept $BUILD_DIR (it holds other files)"
    fi
}

# Removes the folders in the arguments that are empty, in the order given. Never anything else.
remove_empty_folders() {
    for empty_dir in "$@"; do
        rmdir "$empty_dir" 2>/dev/null || true
    done
}

uninstall() {
    step "Removing DarkBurn"
    REMOVED=
    remove_file "$BIN_DIR/darkburn"
    remove_file "$BIN_DIR/darkburn-mcp"
    if [ "$OS" = macos ]; then
        if [ -e "$APP_DIR" ] && [ ! -f "$APP_DIR/Contents/$APP_MARKER" ]; then
            say "  kept $APP_DIR (it was not made by this installer)"
        else
            remove_folder "$APP_DIR"
        fi
    else
        remove_file "$DESKTOP_FILE"
        remove_file "$ICON_DEST"
        remove_file "$MIME_FILE"
        refresh_desktop_caches
        # The folders the installer made for those files, when nothing else is in them.
        remove_empty_folders "$DATA_DIR/icons/hicolor/256x256/apps" "$DATA_DIR/icons/hicolor/256x256" \
            "$DATA_DIR/icons/hicolor" "$DATA_DIR/icons" "$DATA_DIR/mime/packages" "$DATA_DIR/mime" "$DATA_DIR/applications"
    fi
    remove_empty_folders "$BIN_DIR"
    remove_build_dir
    [ -n "$REMOVED" ] || say "  nothing found to remove (was it installed with another DARKBURN_PREFIX or DARKBURN_BUILD_DIR?)"
    if [ "$OS" = linux ] && [ -n "$REMOVED" ]; then
        if [ -e "$DATA_DIR/mime/mime.cache" ] || [ -e "$DATA_DIR/applications/mimeinfo.cache" ]; then
            say "  The desktop's lookup tables under $DATA_DIR/mime and"
            say "  $DATA_DIR/applications stay: other programs share them."
        fi
    fi
    say ""
    if [ -d "$SETTINGS_DIR" ]; then
        say "Your settings are kept in $SETTINGS_DIR"
        say "Delete that folder too if you want them gone."
    else
        say "Settings would be in $SETTINGS_DIR; there are none."
    fi
    if [ "$OS" = linux ] && [ -d "$DATA_DIR/$APP_ID" ]; then
        say "A start with --safe-mode leaves a folder of its own, which is kept too:"
        say "  $DATA_DIR/$APP_ID"
    fi
    say "Rust and the system packages are left as they are."
}

# ---------------------------------------------------------------------------------------------

main() {
    parse_args "$@"
    if [ "$ACTION" = help ]; then
        usage
        return 0
    fi
    settings
    if [ "$ACTION" = uninstall ]; then
        uninstall
        return 0
    fi

    say "DarkBurn installer (pre-release) for $OS $MACHINE"
    if [ "$OS" = macos ]; then
        say "On macOS DarkBurn has been built and tested on a build server only: nobody has used it, or this installer, on a Mac yet."
    fi
    if [ -n "${SUDO_USER:-}" ] && [ "$(id -u 2>/dev/null || true)" = 0 ]; then
        warn "this is running under sudo, so DarkBurn is installed for root only ($HOME). It needs no sudo."
    fi

    TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/darkburn-install.XXXXXX") || die "could not make a temporary folder."
    trap 'rm -rf "$TMP_DIR"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # Before a long build, not after it.
    if [ "$OS" = macos ]; then check_app_dir; fi

    BUILT_HERE=
    if ! try_prebuilt; then
        if [ -n "$PREBUILT_WHY" ]; then
            step "$PREBUILT_WHY Building from source."
        else
            step "Building from source."
        fi
        build_from_source
        BUILT_HERE=1
    fi

    step "Installing into $PREFIX"
    if [ "$OS" = macos ]; then install_macos; else install_linux; fi
    finish
}

main "$@"
# The closing brace of the group that opens above `set -eu`. Nothing may come after it.
}
