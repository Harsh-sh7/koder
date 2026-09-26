#!/bin/sh
# A node the engine can run on, from what this machine already has.
#
# dsh declares `node ^22.19 or >=24` (its package.json `engines`). This script
# answers "which interpreter should we use" without installing anything: the
# `node` on PATH when it is new enough, then the usual homebrew `node@24` keg,
# then a copy this repository provisioned into `.toolchain/` on an earlier run
# (see provision-node.sh, which `make setup` runs).
#
# Prints the absolute path and exits 0, or prints nothing and exits 1 — the
# caller decides whether that means "provision one" or "refuse".
#
#   HARNESS_NODE             an interpreter to use first, whatever is on PATH
#   HARNESS_NODE_VERSION     the version provision-node.sh installs
set -u

root=$(cd "$(dirname "$0")/../.." && pwd)

# The engine's own range, in one line: 22.19 and up, or any 24 and up.
new_enough() {
    [ -n "$1" ] && [ -x "$1" ] || return 1
    "$1" -e 'const [a, b] = process.versions.node.split(".").map(Number); process.exit(a > 22 || (a === 22 && b >= 19) ? 0 : 1)' 2>/dev/null
}

for candidate in \
    "${HARNESS_NODE:-}" \
    "$(command -v node 2>/dev/null || true)" \
    /opt/homebrew/opt/node@24/bin/node \
    /usr/local/opt/node@24/bin/node \
    "$root/.toolchain/node/bin/node"
do
    if new_enough "$candidate"; then
        echo "$candidate"
        exit 0
    fi
done

exit 1
