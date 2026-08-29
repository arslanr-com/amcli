#!/bin/sh
# Install the amcli binary.
#
# CONTRACT: stdout carries the absolute path of the binary and nothing else.
# Everything a human reads goes to stderr. An agent can therefore rely on
#
#     AMCLI_BIN="$(sh install.sh)"
#
# and use "$AMCLI_BIN" for the rest of the session, which matters because a
# freshly installed binary is usually not yet on the calling shell's PATH.
#
# POSIX sh on purpose. Agents run in Alpine containers, which have no bash, so
# there is no [[ ]], no array, no `local`, no `echo -e` and no `pipefail` here.
#
# Never uses sudo. Never edits a shell rc file. Never prompts.
#
# Safe to run at the start of every session: if the newest release is what
# is already installed it downloads nothing and just prints the path, and if
# the network is down it keeps whatever is installed rather than failing. So
# "check amcli is there and up to date" is this one line, every time.
#
#   AMCLI_VERSION=v0.1.0         install that tag instead of the newest
#   AMCLI_INSTALL_DIR=/path/bin  default ~/.local/bin
#   AMCLI_DRY_RUN=1              report what would happen, download nothing
#
# What it trusts, this being a script that downloads a binary and runs it:
#
#   - One host, written into the script: https://github.com/arslanr-com/amcli.
#     No variable can point it elsewhere. AMCLI_VERSION selects a tag and is
#     checked to be one, so it cannot climb out of the URL path.
#   - HTTPS for every request, redirects included.
#   - The archive is checked against the release's SHA256SUMS before it is
#     opened, and only the binary is taken out of it, by name. There is no
#     flag to skip that check and no path that reaches an unverified binary.
#   - A source build is of the same release tag, never of the branch.
#
# What none of that protects against is the release itself: whoever can
# publish a release to that repository publishes its SHA256SUMS in the same
# breath. The guarantee is "exactly the bytes that release published", which
# is what `cargo install` and `npm i` give too, and is not a stronger claim.

set -eu

REPO=arslanr-com/amcli
BASE=https://github.com/$REPO
DIR=${AMCLI_INSTALL_DIR:-${HOME:-.}/.local/bin}

say() { printf '%s\n' "$*" >&2; }
die() { printf 'amcli install: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

TARGET=
TAG=
# amcli.exe under Git Bash; set by detect_target.
BIN=amcli
TMP=

# Must end in a success: an EXIT trap whose last command fails replaces the
# script's exit status with that failure, so a clean install would report 1.
cleanup() {
    if [ -n "$TMP" ] && [ -d "$TMP" ]; then rm -rf "$TMP"; fi
    return 0
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------- platform

# Sets TARGET, and BIN when the executable is not simply called `amcli`.
# Returns 1 for a platform with no prebuilt binary, which the caller turns into
# the cargo fallback rather than an error.
detect_target() {
    os=$(uname -s 2>/dev/null || echo unknown)
    arch=$(uname -m 2>/dev/null || echo unknown)
    case $os in
    Darwin)
        # A shell spawned from an x86_64 Node or editor reports x86_64 even on
        # Apple Silicon, and we would silently install the slow binary. Ask the
        # kernel what the hardware actually is.
        if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
            arch=arm64
        fi
        case $arch in
        arm64 | aarch64) TARGET=aarch64-apple-darwin ;;
        x86_64) TARGET=x86_64-apple-darwin ;;
        *) return 1 ;;
        esac
        ;;
    Linux)
        case $arch in
        aarch64 | arm64) TARGET=aarch64-unknown-linux-musl ;;
        x86_64) TARGET=x86_64-unknown-linux-musl ;;
        *) return 1 ;;
        esac
        ;;
    MINGW* | MSYS* | CYGWIN*)
        # Git Bash, MSYS and Cygwin run the native Windows build; only the
        # file name differs.
        BIN=amcli.exe
        case $arch in
        x86_64) TARGET=x86_64-pc-windows-msvc ;;
        *) return 1 ;;
        esac
        ;;
    *) return 1 ;;
    esac
    return 0
}

# ---------------------------------------------------------------- transport

# GNU wget follows a redirect from https down to plain http unless --https-only
# says otherwise. Busybox wget, which is what Alpine ships and therefore what a
# lot of agent containers have, does not know that option and exits on it, so
# ask once rather than assume either way. Only when there is no curl: curl says
# the same thing with --proto '=https' and is tried first.
HTTPS_ONLY=
if ! have curl && have wget && wget --help 2>&1 | grep -q -- '--https-only'; then
    HTTPS_ONLY=1
fi

# Without -f a 404 page is written into the tarball and tar then reports
# "not in gzip format", which is the single most common naive-installer bug.
# curl's --proto '=https' covers redirects as well as the first request, which
# is the whole reason for spelling it out.
fetch() {
    if have curl; then
        curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
    elif have wget; then
        if [ -n "$HTTPS_ONLY" ]; then
            wget -q --https-only -O "$2" "$1"
        else
            wget -q -O "$2" "$1"
        fi
    else
        return 127
    fi
}

# Sets TAG from the redirect that /releases/latest performs. Deliberately not
# api.github.com: that is 60 requests/hour per IP unauthenticated, shared
# across a NAT and across every Actions runner.
resolve_tag() {
    url=
    if have curl; then
        url=$(curl -sSL --proto '=https' -o /dev/null -w '%{url_effective}' "$BASE/releases/latest" 2>/dev/null || true)
    elif have wget; then
        url=$(wget --max-redirect=0 -S -O /dev/null "$BASE/releases/latest" 2>&1 |
            sed -n 's/^[[:space:]]*[Ll]ocation:[[:space:]]*//p' | tr -d '\r' | tail -1)
    else
        return 127
    fi
    TAG=${url##*/}
    # With no releases at all GitHub redirects to the list page, so the last
    # segment is "releases" rather than a tag. That is the day-one state of a
    # fresh repository and must route to the cargo fallback, not to a 404.
    # Clear TAG on the way out: a caller that ignores the status must not end
    # up building or downloading something named after a redirect.
    case $TAG in
    v[0-9]*) return 0 ;;
    *) TAG=; return 1 ;;
    esac
}

# ---------------------------------------------------------------- checksums

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif have openssl; then
        openssl dgst -sha256 "$1" | sed 's/.*[= ]//'
    else
        return 1
    fi
}

# `sha256sum -c` needs the file present under the recorded name in the current
# directory and macOS `shasum -c` differs again, so compare the strings here.
verify() {
    file=$1
    sums=$2
    name=$3
    want=$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1; exit }' "$sums")
    [ -n "$want" ] || die "$name is not listed in SHA256SUMS"
    got=$(sha256_of "$file") || die "no sha256sum, shasum or openssl found, so
the download cannot be verified. Install any one of them and re-run. There is
deliberately no flag that installs an unverified binary."
    # sha256_of ends in a pipe, so a tool that fails still exits 0 through cut
    # with nothing to show for it. An empty hash is a failure, not a mismatch.
    [ -n "$got" ] || die "could not compute the sha256 of $name"
    [ "$got" = "$want" ] || die "checksum mismatch for $name
  expected $want
  got      $got"
}

# ---------------------------------------------------------------- fallback

# Reached when there is no prebuilt binary for this platform, when no release
# exists yet, or when any download 404s.
#
# It builds the tag, not the branch. A source install should be the same
# version a binary install would have been, and the default branch is a moving
# target that nobody reviewed as a release. TAG is empty only on a repository
# with no releases at all, which is the one case with nothing to pin to.
cargo_fallback() {
    say "$1"
    have cargo || die "no prebuilt binary for this platform and cargo is not
installed. Install Rust from https://rustup.rs and re-run this script, or run:
  cargo install --git $BASE --locked amcli-cli"

    mkdir -p "$DIR"
    TMP=$(mktemp -d "$DIR/.amcli-install.XXXXXX")
    built=
    if [ -n "$TAG" ]; then
        say "building amcli $TAG from source with cargo (a few minutes)..."
        cargo install --git "$BASE" --tag "$TAG" --locked --root "$TMP" amcli-cli >&2 && built=1
    else
        say "building from source with cargo (a few minutes)..."
        cargo install --git "$BASE" --locked --root "$TMP" amcli-cli >&2 && built=1
    fi
    [ -n "$built" ] || die "cargo could not build amcli. amcli needs Rust 1.90
or newer (edition 2024); if yours is older, run \`rustup update\` and try again."
    install_from "$TMP/bin/$BIN"
}

# ---------------------------------------------------------------- install

install_from() {
    mkdir -p "$DIR"
    chmod 755 "$1"
    # Same-filesystem rename: TMP lives inside DIR precisely so this cannot
    # fail by crossing a mount point, and so two installs cannot interleave.
    mv -f "$1" "$DIR/$BIN"

    resolved=$DIR/$BIN
    case $resolved in
    /*) ;;
    *) resolved=$(cd "$DIR" && pwd)/$BIN ;;
    esac

    # A stale binary earlier in PATH turns "it installed fine but behaves like
    # the old version" into an unexplainable bug, so name it.
    onpath=$(command -v amcli 2>/dev/null || true)
    if [ -n "$onpath" ] && [ "$onpath" != "$resolved" ]; then
        say "warning: $onpath comes earlier in PATH and will shadow this install."
        say "         Use the absolute path below, or remove the other copy."
    elif [ -z "$onpath" ]; then
        say "note: $DIR is not on PATH. Use the absolute path below, or add it:"
        say "         export PATH=\"$DIR:\$PATH\""
    fi

    say "installed $("$resolved" --version 2>/dev/null || echo amcli)"
    printf '%s\n' "$resolved"
}

# ---------------------------------------------------------------- main

# AMCLI_VERSION ends up in a download URL and in `cargo install --tag`, so it
# is checked here rather than passed along: a slash in it would aim the
# download at some other path on the host.
if [ -n "${AMCLI_VERSION:-}" ]; then
    case $AMCLI_VERSION in
    *[!A-Za-z0-9._-]*) die "AMCLI_VERSION must be a release tag such as v0.6.0" ;;
    v[0-9]*) TAG=$AMCLI_VERSION ;;
    *) die "AMCLI_VERSION must be a release tag such as v0.6.0" ;;
    esac
fi

rc=0
detect_target || rc=$?
if [ "$rc" != 0 ]; then
    # Resolve the tag first so the source build is of a release rather than of
    # whatever the branch holds today.
    if [ -z "$TAG" ]; then resolve_tag || :; fi
    cargo_fallback "no prebuilt binary for $(uname -s)/$(uname -m)."
    exit 0
fi

# The version already installed at the target path, as `vX.Y.Z`, or empty.
installed_tag() {
    v=$("$DIR/$BIN" --version 2>/dev/null | awk '{ print $2; exit }') || return 1
    [ -n "$v" ] || return 1
    printf 'v%s\n' "$v"
}

if [ -z "$TAG" ] && ! resolve_tag; then
    # No release, or no network. If a binary is installed, that is the one to
    # use: a session that starts offline should not lose a working amcli.
    if have_installed=$(installed_tag); then
        say "could not reach $BASE; keeping installed amcli $have_installed"
        printf '%s\n' "$DIR/$BIN"
        exit 0
    fi
    if [ "${AMCLI_DRY_RUN:-0}" = 1 ]; then
        say "target:  $TARGET"
        say "release: none published yet, would build with cargo"
        printf '%s\n' "$DIR/$BIN"
        exit 0
    fi
    cargo_fallback "no published release found for $REPO yet."
    exit 0
fi

# Already current: nothing to download.
if [ "$(installed_tag 2>/dev/null || true)" = "$TAG" ]; then
    say "amcli $TAG is already installed"
    printf '%s\n' "$DIR/$BIN"
    exit 0
fi

ASSET=amcli-$TAG-$TARGET.tar.gz
URL=$BASE/releases/download/$TAG/$ASSET

if [ "${AMCLI_DRY_RUN:-0}" = 1 ]; then
    say "target:  $TARGET"
    say "release: $TAG"
    say "url:     $URL"
    say "install: $DIR/$BIN"
    printf '%s\n' "$DIR/$BIN"
    exit 0
fi

mkdir -p "$DIR"
TMP=$(mktemp -d "$DIR/.amcli-install.XXXXXX")

say "downloading amcli $TAG for $TARGET..."
if ! fetch "$URL" "$TMP/$ASSET"; then
    rm -rf "$TMP"
    TMP=
    cargo_fallback "no $TARGET build in release $TAG."
    exit 0
fi

if ! fetch "$BASE/releases/download/$TAG/SHA256SUMS" "$TMP/SHA256SUMS"; then
    die "release $TAG has no SHA256SUMS, so the download cannot be verified."
fi

verify "$TMP/$ASSET" "$TMP/SHA256SUMS" "$ASSET"

# Verified before it is opened, and only the member named here comes out of
# it, so nothing else the archive carries is written to disk: not the LICENSE
# and NOTICE it is meant to hold, and not a path trying to climb out of $TMP.
tar -xzf "$TMP/$ASSET" -C "$TMP" "$BIN" || die "could not unpack $BIN from $ASSET"
[ -f "$TMP/$BIN" ] || die "$ASSET does not contain $BIN"

install_from "$TMP/$BIN"
