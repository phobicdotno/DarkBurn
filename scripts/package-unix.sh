#!/bin/sh
# Makes the DarkBurn package for Linux or macOS:
#   dist/darkburn-<version>-<linux|macos>-<arch>.tar.gz   and   <that name>.sha256
#
#   scripts/package-unix.sh                      build for this computer, then package
#   scripts/package-unix.sh --target <triple>    build for this Rust target, then package
#   scripts/package-unix.sh --skip-build         package the programs that are already built
#   scripts/package-unix.sh --bin-dir <folder>   package the two programs in this folder
#   A bare argument works too: a folder means --bin-dir, anything else means --target.
#
# Targets and the names they get (the names scripts/install.sh asks a release for):
#   x86_64-unknown-linux-gnu    linux-x86_64         aarch64-apple-darwin   macos-arm64
#   aarch64-unknown-linux-gnu   linux-aarch64        x86_64-apple-darwin    macos-x86_64
#
# The archive holds one folder, darkburn-<version>-<os>-<arch>, with the programs `darkburn`
# and `darkburn-mcp`, the icon darkburn-256.png, README.txt, the licences (LICENSE-MIT,
# LICENSE-APACHE, ATTRIBUTION.md, licenses/) and THIRD-PARTY-LICENSES.txt. The version is
# [workspace.package] version in the root Cargo.toml. The same input gives the same archive:
# the files are packed in sorted order, owned by root, with the time of the last commit.
# Before it finishes the script unpacks the archive again and starts both programs from it,
# when this computer can run them.
#
# Other options
#   --require-licences   Stop when cargo-about is not installed. Without this option the
#                        third-party licence list is left out, with a message. The release
#                        workflow uses it, so a release always has the list.
#   --require-start      Stop when this computer cannot start the packed programs. Without
#                        this option they are then packed unstarted, with a message. The
#                        release workflow uses it, so a release was always started once.
#   --help               Show the help text.
#
# Environment
#   DIST                Where the package goes               [dist]
#   CARGO_TARGET_DIR    Cargo's target folder                [target]
#   SOURCE_DATE_EPOCH   The time stamp of the packed files   [the time of the last commit]
#
# Needs sh, tar, gzip and sha256sum, shasum or openssl. Needs cargo unless the programs are
# already built, and cargo-about for the licence list:
#   cargo install cargo-about --locked --features cli
#
# This is plain POSIX sh, like install.sh, and not bash: it then runs the same with dash on
# Linux and with the old bash that macOS has as /bin/sh.

set -eu

say() { printf '%s\n' "$*"; }
step() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat <<'EOF'
Makes the DarkBurn package for Linux or macOS:
  dist/darkburn-<version>-<linux|macos>-<arch>.tar.gz and <that name>.sha256

Usage
  scripts/package-unix.sh [--target <triple>] [--skip-build] [--bin-dir <folder>]
                          [--require-licences] [--require-start] [--help]

  (nothing)            Build for this computer, then package.
  --target <triple>    Build for this Rust target, then package. One of
                       x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
                       aarch64-apple-darwin, x86_64-apple-darwin.
  --skip-build         Package the programs that are already built in the target folder.
  --bin-dir <folder>   Package the programs darkburn and darkburn-mcp in this folder.
  --require-licences   Stop when cargo-about is not installed, instead of leaving the
                       third-party licence list out.
  --require-start      Stop when this computer cannot start the packed programs, instead
                       of packing them unstarted.
  A bare argument works too: a folder means --bin-dir, anything else means --target.

Environment
  DIST                Where the package goes               [dist]
  CARGO_TARGET_DIR    Cargo's target folder                [target]
  SOURCE_DATE_EPOCH   The time stamp of the packed files   [the time of the last commit]
EOF
}

# ---------------------------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------------------------

# The value of `key = value` ($3) in section [$2] of the TOML file $1, without its quotes.
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
            sub(/[ \t]+#.*$/, "", v)
            gsub(/^[ \t]+|[ \t]+$/, "", v)
            gsub(/^"|"$/, "", v)
            print v
            exit
        }
    ' "$1"
}

# Makes $1 absolute (relative paths are taken from the current folder).
absolute_path() {
    case "$1" in
        /*) say "$1" ;;
        *) say "$PWD/$1" ;;
    esac
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

# The Rust target of this computer: what rustc builds for, else what uname says.
host_triple() {
    host_guess=
    if has rustc; then
        host_guess=$(cd "$ROOT" && rustc -vV 2>/dev/null | sed -n 's/^host: //p') || host_guess=
    fi
    if [ -z "$host_guess" ]; then
        case "$(uname -s)/$(uname -m)" in
            Linux/x86_64 | Linux/amd64) host_guess=x86_64-unknown-linux-gnu ;;
            Linux/aarch64 | Linux/arm64) host_guess=aarch64-unknown-linux-gnu ;;
            Darwin/x86_64) host_guess=x86_64-apple-darwin ;;
            Darwin/arm64) host_guess=aarch64-apple-darwin ;;
        esac
    fi
    say "$host_guess"
}

# True when this computer can start a program built for $OS and $ARCH.
can_run_here() {
    case "$(uname -s)" in
        Linux)
            [ "$OS" = linux ] || return 1
            case "$(uname -m)" in
                x86_64 | amd64) [ "$ARCH" = x86_64 ] ;;
                aarch64 | arm64) [ "$ARCH" = aarch64 ] ;;
                *) return 1 ;;
            esac
            ;;
        Darwin)
            [ "$OS" = macos ] || return 1
            if [ "$ARCH" = arm64 ]; then
                [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]
            else
                # An Intel Mac, or an Apple silicon Mac with Rosetta.
                [ "$(uname -m)" = x86_64 ] || arch -x86_64 /usr/bin/true >/dev/null 2>&1
            fi
            ;;
        *) return 1 ;;
    esac
}

# Runs "$@" with nothing on its input and its output in $RUN_LOG. A program that has not
# finished after $1 seconds is stopped, and that counts as a failure. Sets run_status.
run_limited() {
    run_limit=$1
    shift
    "$@" </dev/null >"$RUN_LOG" 2>&1 &
    run_pid=$!
    (
        sleep "$run_limit"
        kill "$run_pid" 2>/dev/null
    ) >/dev/null 2>&1 &
    run_watch=$!
    if wait "$run_pid"; then run_status=0; else run_status=$?; fi
    kill "$run_watch" 2>/dev/null || true
    wait "$run_watch" 2>/dev/null || true
    return "$run_status"
}

# The newest glibc version the Linux program $1 asks for (for example 2.35), or nothing.
# Weak references (a "w" in the second column) do not count: the program starts without them.
glibc_needed() {
    has objdump || return 0
    objdump -T "$1" 2>/dev/null | awk '
        $2 == "w" { next }
        {
            for (i = 1; i <= NF; i++) {
                word = $i
                gsub(/[()]/, "", word)
                if (word !~ /^GLIBC_[0-9]+(\.[0-9]+)+$/) continue
                sub(/^GLIBC_/, "", word)
                n = split(word, part, ".")
                rank = part[1] * 1000000 + part[2] * 1000 + (n > 2 ? part[3] : 0)
                if (rank > best_rank) { best_rank = rank; best = word }
            }
        }
        END { if (best != "") print best }
    ' || true
}

# The oldest macOS the program $1 is built for (for example 11.0), or nothing.
macos_needed() {
    has otool || return 0
    otool -l "$1" 2>/dev/null | awk '
        $1 == "cmd" { old = ($2 == "LC_VERSION_MIN_MACOSX") }
        $1 == "minos" { print $2; exit }
        old && $1 == "version" { print $2; exit }
    ' || true
}

# Stops unless the darkburn-mcp program $1 starts here and is of the version in Cargo.toml.
# $2 names the program in the message. scripts/install.sh starts darkburn-mcp like this
# before it installs a release.
same_version() {
    version_says=$("$1" --version 2>&1) || die "$2 does not start here: $version_says"
    [ "$version_says" = "darkburn-mcp $VERSION" ] ||
        die "$2 says \"$version_says\", but the version in Cargo.toml is $VERSION.
$VERSION_HINT"
    say "    $version_says"
}

# ---------------------------------------------------------------------------------------------
# Arguments and settings
# ---------------------------------------------------------------------------------------------

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)

TRIPLE=
BIN_DIR=
SKIP_BUILD=
REQUIRE_LICENCES=
REQUIRE_START=
while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --skip-build) SKIP_BUILD=1 ;;
        --require-licences) REQUIRE_LICENCES=1 ;;
        --require-start) REQUIRE_START=1 ;;
        --target)
            [ $# -ge 2 ] || die "--target needs a Rust target after it."
            TRIPLE=$2
            shift
            ;;
        --target=*) TRIPLE=${1#*=} ;;
        --bin-dir)
            [ $# -ge 2 ] || die "--bin-dir needs a folder after it."
            BIN_DIR=$2
            shift
            ;;
        --bin-dir=*) BIN_DIR=${1#*=} ;;
        -*) die "unknown option: $1 (try --help)" ;;
        *)
            if [ -d "$1" ]; then BIN_DIR=$1; else TRIPLE=$1; fi
            ;;
    esac
    shift
done
# Programs that are handed over are packaged as they are.
[ -z "$BIN_DIR" ] || SKIP_BUILD=1
# What to say when the programs turn out to be of another version.
if [ -n "$BIN_DIR" ]; then
    VERSION_HINT="The programs in $BIN_DIR were built from another version: hand over programs of this version."
elif [ -n "$SKIP_BUILD" ]; then
    VERSION_HINT='The programs were built from another version: build again, or drop --skip-build.'
else
    VERSION_HINT='The build did not give programs of this version: the lines above may say why.'
fi

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
VERSION=$(toml_value "$ROOT/Cargo.toml" workspace.package version)
[ -n "$VERSION" ] || die "could not read [workspace.package] version from Cargo.toml."

HOST_TRIPLE=$(host_triple)
[ -n "$TRIPLE" ] || TRIPLE=$HOST_TRIPLE
[ -n "$TRIPLE" ] || die "could not tell what this computer is. Name the target with --target."
case "$TRIPLE" in
    x86_64-unknown-linux-gnu) OS=linux ARCH=x86_64 OS_NAME=Linux ;;
    aarch64-unknown-linux-gnu) OS=linux ARCH=aarch64 OS_NAME=Linux ;;
    x86_64-apple-darwin) OS=macos ARCH=x86_64 OS_NAME=macOS ;;
    aarch64-apple-darwin) OS=macos ARCH=arm64 OS_NAME=macOS ;;
    *) die "there is no package for the target $TRIPLE.
The targets are x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, aarch64-apple-darwin
and x86_64-apple-darwin. The Windows packages come from scripts/build-installer.ps1 and
scripts/build-portable.ps1." ;;
esac

NAME="darkburn-$VERSION-$OS-$ARCH"
DIST=$(absolute_path "${DIST:-$ROOT/dist}")
TARGET_DIR=$(absolute_path "${CARGO_TARGET_DIR:-$ROOT/target}")
# Everything this run makes on the way lives in one folder of its own, under the target folder.
WORK="$TARGET_DIR/unix-package/$NAME"
STAGE="$WORK/$NAME"
CHECK="$WORK/check"
RUN_LOG="$WORK/run.log"
# The archive is made and checked in the work folder. It reaches DIST only when it passed.
PACKED="$WORK/$NAME.tar.gz"
ARCHIVE="$DIST/$NAME.tar.gz"

has tar || die "tar is not installed."
has gzip || die "gzip is not installed."
[ -n "$(sha256_of "$ROOT/Cargo.toml")" ] || die "no sha256sum, shasum or openssl here to write the checksum with."

say "DarkBurn $VERSION for $OS_NAME $ARCH ($TRIPLE)"

# Asked before the build, which takes a while.
if [ -n "$REQUIRE_START" ] && ! can_run_here; then
    hint=
    [ "$OS/$ARCH" != macos/x86_64 ] || [ "$(uname -s)" != Darwin ] ||
        hint='
On an Apple silicon Mac, Rosetta starts them: softwareupdate --install-rosetta --agree-to-license'
    die "this computer cannot start $OS_NAME $ARCH programs, and --require-start asks for that.$hint"
fi

# ---------------------------------------------------------------------------------------------
# The programs
# ---------------------------------------------------------------------------------------------

if [ -z "$SKIP_BUILD" ]; then
    has cargo || die "cargo is not installed. Install Rust from https://rustup.rs, or hand over programs that are already built with --bin-dir."
    step "cargo build --release ($TRIPLE)"
    (cd "$ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --locked -p darkburn -p darkburn-mcp --target "$TRIPLE") ||
        die "the build failed; the lines above say why."
fi

if [ -n "$BIN_DIR" ]; then
    [ -d "$BIN_DIR" ] || die "$BIN_DIR is not a folder."
    BIN_DIR=$(CDPATH='' cd -- "$BIN_DIR" && pwd)
else
    # With --target the programs are in target/<triple>/release. A plain `cargo build --release`
    # for this computer puts them in target/release.
    BIN_DIR="$TARGET_DIR/$TRIPLE/release"
    if [ ! -f "$BIN_DIR/darkburn" ] && [ "$TRIPLE" = "$HOST_TRIPLE" ] && [ -f "$TARGET_DIR/release/darkburn" ]; then
        BIN_DIR="$TARGET_DIR/release"
    fi
fi
for program in darkburn darkburn-mcp; do
    [ -f "$BIN_DIR/$program" ] || die "missing $BIN_DIR/$program (build first, or drop --skip-build)."
done

# The right kind of program for the name it gets. Skipped where `file` is not installed.
if has file; then
    for program in darkburn darkburn-mcp; do
        kind=$(file -b "$BIN_DIR/$program" 2>/dev/null || true)
        case "$OS/$ARCH: $kind" in
            "linux/x86_64: ELF 64-bit"*x86-64* | "linux/aarch64: ELF 64-bit"*aarch64* | \
                "macos/x86_64: Mach-O 64-bit"*x86_64* | "macos/arm64: Mach-O 64-bit"*arm64*)
                say "ok $program: ${kind%%,*}"
                ;;
            *) die "$BIN_DIR/$program is not a $OS_NAME $ARCH program: $kind" ;;
        esac
    done
fi

# The package is named after the version, so the programs must be of that version. Asked
# here, before anything is copied or packed.
if can_run_here; then
    same_version "$BIN_DIR/darkburn-mcp" "$BIN_DIR/darkburn-mcp"
fi

# ---------------------------------------------------------------------------------------------
# The folder that goes into the archive
# ---------------------------------------------------------------------------------------------

step "Collecting the files"
# The work folder goes away however this script ends, so a stop on the way leaves nothing big
# behind.
trap 'rm -rf "$WORK"' EXIT
trap 'exit 130' HUP INT TERM
rm -rf "$WORK"
mkdir -p "$STAGE/licenses" "$CHECK"
cp "$BIN_DIR/darkburn" "$BIN_DIR/darkburn-mcp" "$STAGE/"
cp "$ROOT/assets/icons/app/darkburn-256.png" "$STAGE/"
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/ATTRIBUTION.md" "$STAGE/"
cp "$ROOT/licenses/"* "$STAGE/licenses/"

# The release profile keeps line tables in the programs (debug = "line-tables-only") and does
# not strip them. Here they are taken out of the packed copies; the names of the functions
# stay. When [profile.release] strips by itself there is nothing left to do.
profile_strip=$(toml_value "$ROOT/Cargo.toml" profile.release strip)
case "$profile_strip" in
    true | symbols | debuginfo)
        say "    [profile.release] strip = $profile_strip: the programs are packed as built"
        ;;
    *)
        for program in darkburn darkburn-mcp; do
            stripped=
            if [ "$OS" = macos ]; then
                if strip -S "$STAGE/$program" 2>/dev/null; then stripped=1; fi
            else
                for strip_tool in strip "$ARCH-linux-gnu-strip" llvm-strip; do
                    if has "$strip_tool" && "$strip_tool" --strip-debug "$STAGE/$program" 2>/dev/null; then
                        stripped=1
                        break
                    fi
                done
            fi
            if [ -n "$stripped" ]; then
                say "    $program: debug information taken out"
            else
                warn "no strip tool here for $OS_NAME $ARCH programs: $program is packed as built."
            fi
        done
        ;;
esac
# macOS on Apple silicon starts only signed programs. The linker signs them for this computer
# (ad hoc); after a change to the file that signature has to be made again.
if [ "$OS" = macos ] && has codesign; then
    for program in darkburn darkburn-mcp; do
        codesign --force --sign - "$STAGE/$program" >/dev/null 2>&1 ||
            die "could not sign $program (codesign --force --sign -)."
    done
fi

# Third-party licences, as scripts/package-common.ps1 writes them for Windows. The list depends
# on the target: packaging/about.toml names the Windows one, and --target puts this package's
# target in its place. The accepted licences are the same.
if has cargo && { has cargo-about || [ -x "${CARGO_HOME:-${HOME:-}/.cargo}/bin/cargo-about" ]; }; then
    step "cargo about generate (THIRD-PARTY-LICENSES.txt)"
    (cd "$ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo about generate --locked --workspace --target "$TRIPLE" \
        --config packaging/about.toml packaging/about.hbs -o "$STAGE/THIRD-PARTY-LICENSES.txt") ||
        die "cargo about failed; the lines above say why.
A dependency under a licence that packaging/about.toml does not list stops here on purpose."
    [ -s "$STAGE/THIRD-PARTY-LICENSES.txt" ] || die "cargo about wrote an empty THIRD-PARTY-LICENSES.txt."
    licence_line='  THIRD-PARTY-LICENSES.txt     the licences of the libraries DarkBurn is built with'
elif [ -n "$REQUIRE_LICENCES" ]; then
    die "cargo-about is not installed, so THIRD-PARTY-LICENSES.txt cannot be written.
Install it with: cargo install cargo-about --locked --features cli"
else
    say "    cargo-about is not installed: THIRD-PARTY-LICENSES.txt is left out of this package."
    say "    For a package with the list: cargo install cargo-about --locked --features cli"
    licence_line='  (THIRD-PARTY-LICENSES.txt, the licences of the libraries, is not in this package.)'
fi

# What the programs need from the system, read from the programs themselves.
if [ "$OS" = linux ]; then
    needs_version=$(glibc_needed "$STAGE/darkburn")
    if [ -n "$needs_version" ]; then
        say "    darkburn needs glibc $needs_version or newer"
        needs_text="  Linux on $ARCH with glibc $needs_version or newer (ldd --version shows yours),"
    else
        needs_text="  Linux on $ARCH,"
    fi
    needs_text="$needs_text
  a desktop with X11 or Wayland, and a Vulkan or OpenGL graphics driver.
  Under X11 it also needs libxkbcommon-x11 (Debian and Ubuntu: sudo apt install
  libxkbcommon-x11-0). The install line above tells you when it is missing."
    install_text='  It puts both programs in ~/.local/bin and adds a menu entry and an icon.'
    status_extra=
else
    needs_version=$(macos_needed "$STAGE/darkburn")
    if [ -n "$needs_version" ]; then
        say "    darkburn needs macOS $needs_version or newer"
        needs_text="  macOS $needs_version or newer on $ARCH."
    else
        needs_text="  macOS on $ARCH."
    fi
    needs_text="$needs_text
  The programs are not signed with an Apple developer certificate, so macOS may refuse to
  open a copy that came through a web browser. The install line above does not go through
  one. For a copy that did, run this in the folder: xattr -dr com.apple.quarantine ."
    install_text='  It makes DarkBurn.app in ~/Applications, with links to both programs in ~/.local/bin.'
    status_extra='
On macOS DarkBurn has been built and tested on a build server only: nobody has used it on a Mac yet.'
fi

cat >"$STAGE/README.txt" <<EOF
DarkBurn $VERSION for $OS_NAME ($ARCH)

DarkBurn is a design and control program for laser cutters and engravers.
Pre-release: DarkBurn has not yet run a job on a real machine.$status_extra

In this folder
  darkburn                     the program
  darkburn-mcp                 the MCP server: it connects an AI agent, such as Claude Code
                               or Claude Desktop, to the running DarkBurn window
  darkburn-256.png             the icon
  LICENSE-MIT, LICENSE-APACHE  DarkBurn's licence: MIT or Apache-2.0, as you choose
  ATTRIBUTION.md, licenses/    credit for the code DarkBurn reuses
$licence_line

Run
  ./darkburn
  The MCP server is off until you turn it on in Settings > AI agents (MCP).
  ./darkburn-mcp --print-config prints the lines that register it with Claude Code and
  Claude Desktop.

Install
  curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh | sh
  This installs the newest release for you alone, without sudo.
$install_text
  To remove it again, put this at the end of the same line:  -s -- --uninstall

Needs
$needs_text

The full README, with pictures: https://github.com/phobicdotno/DarkBurn#readme
EOF

chmod 755 "$STAGE" "$STAGE/licenses" "$STAGE/darkburn" "$STAGE/darkburn-mcp"
find "$STAGE" -type f ! -name darkburn ! -name darkburn-mcp -exec chmod 644 {} +

# One time stamp for everything: SOURCE_DATE_EPOCH, else the last commit, else now.
EPOCH=${SOURCE_DATE_EPOCH:-}
epoch_from='SOURCE_DATE_EPOCH is'
if [ -z "$EPOCH" ]; then
    # Git's answer, or what it says when it has none.
    EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>&1 || true)
    epoch_from='git said'
fi
case "$EPOCH" in
    '' | *[!0-9]*)
        warn "the time of the last commit is not known, so the packed files get the time of now.
This archive differs from the next one made from the same input. $epoch_from: ${EPOCH:-nothing}"
        EPOCH=$(date +%s)
        ;;
esac
# The date of macOS and the BSDs takes the seconds as -r N, GNU date as -d @N.
case "$(uname -s)" in
    Darwin | *BSD) STAMP=$(date -u -r "$EPOCH" +%Y%m%d%H%M.%S) ;;
    *) STAMP=$(date -u -d "@$EPOCH" +%Y%m%d%H%M.%S) ;;
esac
find "$STAGE" -exec env TZ=UTC touch -t "$STAMP" {} +

# ---------------------------------------------------------------------------------------------
# The archive and its checksum
# ---------------------------------------------------------------------------------------------

step "Packing $NAME.tar.gz"
(cd "$WORK" && find "$NAME" -print | LC_ALL=C sort) >"$WORK/files.list"
# macOS: no ._ files with Finder information beside the real ones.
COPYFILE_DISABLE=1
export COPYFILE_DISABLE
tar_kind=$(tar --version 2>/dev/null | head -n 1 || true)
case "$tar_kind" in
    *GNU\ tar*)
        tar -cf "$WORK/$NAME.tar" --format=ustar --owner=0 --group=0 --numeric-owner --no-recursion \
            -C "$WORK" -T "$WORK/files.list"
        ;;
    *bsdtar*)
        tar -cf "$WORK/$NAME.tar" --format ustar --uid 0 --gid 0 --numeric-owner -n \
            -C "$WORK" -T "$WORK/files.list"
        ;;
    *)
        warn "this tar is neither GNU tar nor bsdtar: the files keep their owner and the order tar gives them."
        tar -cf "$WORK/$NAME.tar" -C "$WORK" "$NAME"
        ;;
esac
gzip -n -9 <"$WORK/$NAME.tar" >"$PACKED"

# ---------------------------------------------------------------------------------------------
# Check the archive the way the installer uses it
# ---------------------------------------------------------------------------------------------

step "Checking the archive"
# An older package of the same name must not be left looking like the result of this run.
rm -f "$ARCHIVE" "$ARCHIVE.sha256"
tar -tzf "$PACKED" | sed 's|/$||' | LC_ALL=C sort >"$WORK/files.packed"
if ! cmp -s "$WORK/files.list" "$WORK/files.packed"; then
    say "collected:"
    cat "$WORK/files.list"
    say "packed:"
    cat "$WORK/files.packed"
    die "the archive does not hold exactly the files that were collected."
fi
tar -xzf "$PACKED" -C "$CHECK"
for program in darkburn darkburn-mcp; do
    if [ ! -f "$CHECK/$NAME/$program" ] || [ ! -x "$CHECK/$NAME/$program" ]; then
        die "$program is missing from the unpacked archive, or is not marked as a program."
    fi
done
[ -f "$CHECK/$NAME/darkburn-256.png" ] || die "darkburn-256.png is missing from the unpacked archive."

if can_run_here; then
    same_version "$CHECK/$NAME/darkburn-mcp" 'the packed darkburn-mcp'
    # The app without a window: it serves MCP on its input and stops when the input ends.
    if run_limited 60 "$CHECK/$NAME/darkburn" --mcp-headless; then
        say "    darkburn --mcp-headless starts and stops"
    else
        cat "$RUN_LOG" >&2 || true
        die "the packed darkburn does not start here (darkburn --mcp-headless, exit code $run_status)."
    fi
elif [ -n "$REQUIRE_START" ]; then
    die "this computer cannot start $OS_NAME $ARCH programs, and --require-start asks for that."
else
    say "    not started: this computer cannot run $OS_NAME $ARCH programs"
fi

# The checksum file: the SHA-256 in hex, two spaces, the file name. `sha256sum -c` reads it,
# and so do the installers.
SUM=$(sha256_of "$PACKED")
[ -n "$SUM" ] || die "could not work out the SHA-256 of the archive."
mkdir -p "$DIST"
rm -f "$ARCHIVE" "$ARCHIVE.sha256"
cp "$PACKED" "$ARCHIVE"
printf '%s  %s\n' "$SUM" "$NAME.tar.gz" >"$ARCHIVE.sha256"
[ "$(sha256_of "$ARCHIVE")" = "$SUM" ] || die "$ARCHIVE is not the archive that was checked."
rm -rf "$WORK"

say ""
say "Package:  $ARCHIVE ($(wc -c <"$ARCHIVE" | tr -d ' ') bytes)"
say "Checksum: $ARCHIVE.sha256"
say "SHA-256:  $SUM"
