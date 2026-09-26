#!/bin/sh
# Runs a command with the node the engine accepts first on PATH — and a pnpm.
#
# `make run`, the plugins' tests, pnpm, and the profile bootstrap all assume an
# interpreter new enough for dsh; pnpm and every process it starts inherit
# whatever is on PATH. This is the single place that guarantee is made: resolve
# or provision a node (node-bin.sh, then provision-node.sh), put it first on
# PATH, and make sure a pnpm resolves too. A machine that had no node at all
# has no pnpm either, and `pnpm install` is the next thing `make setup` does —
# so the shim corepack ships with the resolved node is enabled into
# `.toolchain/shims` (gitignored) when, and only when, no pnpm is otherwise on
# PATH. Falling back to `npm install -g` into `.toolchain/pnpm` covers nodes
# that no longer bundle corepack.
#
#   sh App/scripts/with-node.sh node App/scripts/run-acp.mjs
#   cd Deepseek/deepseek-harness && sh .../with-node.sh pnpm install
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
bin=$(sh "$root/App/scripts/provision-node.sh")
PATH="$(dirname "$bin"):$PATH"

# corepack otherwise asks "Do you want to continue? [Y/n]" on the first
# download when stdin is a terminal, which would stall a non-interactive setup.
COREPACK_ENABLE_DOWNLOAD_PROMPT=0
export COREPACK_ENABLE_DOWNLOAD_PROMPT

shims="$root/.toolchain/shims"
if ! command -v pnpm >/dev/null 2>&1 && [ -x "$shims/pnpm" ]; then
    PATH="$shims:$PATH"
fi
export PATH

if ! command -v pnpm >/dev/null 2>&1 && command -v corepack >/dev/null 2>&1; then
    echo "==> pnpm: none on PATH; enabling pnpm through corepack (once)" >&2
    mkdir -p "$shims"
    if corepack enable pnpm --install-directory "$shims" >&2; then
        PATH="$shims:$PATH"
        export PATH
    fi
fi

if ! command -v pnpm >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
    echo "==> pnpm: installing pnpm 11 into .toolchain (once)" >&2
    if npm install --global --prefix "$root/.toolchain/pnpm" pnpm@11 >&2; then
        PATH="$root/.toolchain/pnpm/bin:$PATH"
        export PATH
    fi
fi

if ! command -v pnpm >/dev/null 2>&1; then
    echo "with-node: this machine has no pnpm and none could be installed" >&2
    echo "with-node: install it by hand (https://pnpm.io/installation) and re-run" >&2
    exit 1
fi

exec "$@"
