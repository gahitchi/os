#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Check the TLA+ models with TLC.
#
#   formal/check.sh            run every check CI runs
#   formal/check.sh <cfg>...   run only these configurations of the revoke model
#
# Needs Java 11 or newer. Downloads the pinned TLA+ tools once, into build/.
set -euo pipefail

TLA_VERSION=1.7.4
TLA_SHA256=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88

root=$(cd "$(dirname "$0")/.." && pwd)
jar="$root/build/tla2tools-$TLA_VERSION.jar"
if [ ! -f "$jar" ]; then
    mkdir -p "$root/build"
    curl -fsSL -o "$jar.part" \
        "https://github.com/tlaplus/tlaplus/releases/download/v$TLA_VERSION/tla2tools.jar"
    echo "$TLA_SHA256  $jar.part" | sha256sum -c --quiet
    mv "$jar.part" "$jar"
fi

workers=${TLC_WORKERS:-auto}
tlc() {
    local dir=$1 cfg=$2
    echo "== $dir/$cfg"
    (cd "$root/formal/$dir" &&
        java -XX:+UseParallelGC -cp "$jar" tlc2.TLC -workers "$workers" \
            -metadir "$root/build/tlc/$dir-${cfg%.cfg}" -config "$cfg" "${dir^}.tla")
}

if [ $# -gt 0 ]; then
    for cfg in "$@"; do tlc revoke "$cfg"; done
    exit 0
fi

# Must pass: safety and liveness on two cores, safety on three.
tlc revoke Small.cfg
tlc revoke ThreeCores.cfg

# Must fail: with a revoke that does only one step per turn, other cores
# can keep it busy for ever. This shows the liveness check has teeth.
echo "== revoke/TooSlow.cfg (expected to fail)"
if out=$(tlc revoke TooSlow.cfg 2>&1); then
    echo "$out"
    echo "TooSlow.cfg passed, but it must find the endless revoke" >&2
    exit 1
fi
if ! grep -q "Temporal properties were violated" <<<"$out"; then
    echo "$out"
    echo "TooSlow.cfg failed for the wrong reason" >&2
    exit 1
fi
echo "TooSlow.cfg: endless revoke found, as expected"
echo "formal: all checks passed"
