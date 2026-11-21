#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Check the TLA+ models with TLC.
#
#   formal/check.sh                  run every check CI runs
#   formal/check.sh <model>/<cfg>... run only these, e.g. sched/Small.cfg
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
    for arg in "$@"; do tlc "${arg%%/*}" "${arg#*/}"; done
    exit 0
fi

# A check that must fail, and for the expected reason: it shows the model
# has teeth.
must_fail() {
    local dir=$1 cfg=$2 reason=$3 out
    echo "== $dir/$cfg (expected to fail)"
    if out=$(tlc "$dir" "$cfg" 2>&1); then
        echo "$out"
        echo "$dir/$cfg passed, but it must fail" >&2
        exit 1
    fi
    if ! grep -q "$reason" <<<"$out"; then
        echo "$out"
        echo "$dir/$cfg failed for the wrong reason" >&2
        exit 1
    fi
    echo "$dir/$cfg: failed as expected ($reason)"
}

# Revoke: safety and liveness on two cores, safety on three. With a revoke
# that does only one step per turn, other cores can keep it busy for ever.
tlc revoke Small.cfg
tlc revoke ThreeCores.cfg
must_fail revoke TooSlow.cfg "Temporal properties were violated"

# The scheduler: safety and liveness on two cores, safety on three. A
# refill that forgets its thread loses a wake-up; lent time not given back
# when its caller was stopped is lost.
tlc sched Small.cfg
tlc sched Medium.cfg
tlc sched Bigger.cfg
must_fail sched LostWakeup.cfg "Invariant NoLostWakeups is violated"
must_fail sched LostLoan.cfg "Invariant LoansAccounted is violated"

# The driver keeper: crash-and-resend and live updates. A restart without
# a device reset lets an old job finish a newer one; a keeper that drops
# unfinished jobs leaves apps waiting.
tlc keeper Small.cfg
must_fail keeper NoReset.cfg "is violated"
must_fail keeper ForgetJobs.cfg "Temporal properties were violated"

# The store's commits (milestone 5): after a power cut at any moment, even
# with a bad spot on the newest front page, the store opens at a commit it
# made and loses nothing it promised. Reusing given-up blocks after one
# commit instead of two, or writing the front page before the blocks are
# flushed, breaks that.
tlc commit Small.cfg
must_fail commit EarlyReuse.cfg "Invariant AlwaysOpens is violated"
must_fail commit NoFlush.cfg "Invariant AlwaysOpens is violated"

echo "formal: all checks passed"
