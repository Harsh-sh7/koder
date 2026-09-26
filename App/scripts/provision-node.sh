#!/bin/sh
# A node the engine can run on, downloading one when the machine has none.
#
# `make setup` calls this before anything else, so a clean machine with an old
# system node — or no node at all — still ends up able to run the engine. The
# release tarball for this platform is fetched from nodejs.org once, its
# SHA-256 checked against the release's own SHASUMS256.txt, and unpacked under
# `.toolchain/` (gitignored). Every later run resolves it from there through
# node-bin.sh, so the download happens once per checkout, not once per command.
#
# This is not a build step and not a container: it installs the interpreter dsh
# already declares, pinned by version and verified by checksum, and only when
# the machine does not already have one. `HARNESS_NODE_VERSION` asks for that
# exact release instead of "whatever is new enough"; `HARNESS_NODE` points at
# an interpreter to use instead of installing anything.
#
# Prints the interpreter's path. Diagnostics go to stderr.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
version="${HARNESS_NODE_VERSION:-24.21.0}"
pinned="${HARNESS_NODE_VERSION:-}"
dest="$root/.toolchain"
name="node-v$version"

version_of() {
    [ -x "$1" ] && "$1" --version 2>/dev/null || true
}

# 1. Anything already usable wins, unless a specific release was asked for —
#    nothing is downloaded twice, and nothing is downloaded at all when the
#    machine can already run the engine.
if [ -z "$pinned" ] && found=$(sh "$root/App/scripts/node-bin.sh"); then
    echo "$found"
    exit 0
fi
if [ -n "$pinned" ] && [ "$(version_of "$dest/node/bin/node")" = "v$version" ]; then
    echo "$dest/node/bin/node"
    exit 0
fi

# 2. Which release to fetch. Node names its builds by OS and CPU.
case "$(uname -s)" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) echo "provision-node: no node ^22.19 or >=24 here, and nodejs.org builds none for $(uname -s) — install one by hand" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    arm64 | aarch64) cpu=arm64 ;;
    x86_64 | amd64) cpu=x64 ;;
    *) echo "provision-node: unrecognised architecture $(uname -m) — install node ^22.19 or >=24 by hand" >&2; exit 1 ;;
esac
archive="$name-$os-$cpu.tar.gz"
url="https://nodejs.org/dist/v$version/$archive"

# 3. Fetch the tarball, verify it against the release's own checksum list, and
#    unpack it. A truncated download or a tampered mirror has to fail here
#    rather than at the first `make run`.
mkdir -p "$dest"
if [ ! -f "$dest/$archive" ]; then
    echo "==> node: installing node v$version into .toolchain (once)" >&2
    if ! curl -fsSL "$url" -o "$dest/$archive.part"; then
        echo "provision-node: could not download $url" >&2
        echo "provision-node: install node ^22.19 or >=24 by hand and re-run 'make setup'" >&2
        exit 1
    fi
    mv "$dest/$archive.part" "$dest/$archive"
fi
if [ ! -f "$dest/SHASUMS256.txt" ]; then
    curl -fsSL "https://nodejs.org/dist/v$version/SHASUMS256.txt" -o "$dest/SHASUMS256.txt" || {
        echo "provision-node: could not fetch the release's SHASUMS256.txt" >&2
        exit 1
    }
fi

want=$(awk -v file="$archive" '$2 == file { print $1 }' "$dest/SHASUMS256.txt")
if [ -z "$want" ]; then
    echo "provision-node: $archive is not listed in the release's SHASUMS256.txt" >&2
    exit 1
fi
if command -v shasum >/dev/null 2>&1; then
    got=$(shasum -a 256 "$dest/$archive" | awk '{ print $1 }')
else
    got=$(sha256sum "$dest/$archive" | awk '{ print $1 }')
fi
if [ "$want" != "$got" ]; then
    echo "provision-node: checksum mismatch for $archive (wanted $want, got $got)" >&2
    echo "provision-node: removing the download; check the network and re-run 'make setup'" >&2
    rm -f "$dest/$archive"
    exit 1
fi

# 4. Unpack beside the archive and put it where node-bin.sh looks. The tarball
#    is kept: re-provisioning a moved checkout then needs no network.
rm -rf "$dest/node" "$dest/$name-$os-$cpu"
tar -xzf "$dest/$archive" -C "$dest"
mv "$dest/$name-$os-$cpu" "$dest/node"

echo "==> node: v$version installed in .toolchain/node (checksum verified)" >&2
echo "$dest/node/bin/node"
