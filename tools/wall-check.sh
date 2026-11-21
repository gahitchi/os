#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# The processor wall (milestone 4b): fail if processor-specific code shows
# up in the kernel outside kernel/src/arch/. Comments may mention a
# processor; code may not.
set -eu
cd "$(dirname "$0")/.."

# Code lines only: drop // comments (doc comments included).
code() {
    find kernel/src -name '*.rs' -not -path 'kernel/src/arch/*' -print0 |
        xargs -0 grep -nH -v -E '^[^:]*:[0-9]+:\s*//' |
        sed -E 's#^([^:]*:[0-9]+:)(.*[^:])//[^"]*$#\1\2#'
}

patterns='asm!|global_asm!|core::arch|target_arch|x86|arch::(x86_64|apic|cpu|gdt|idt|legacy|port|protect|trap)\b|rdtsc|rdmsr|wrmsr|cpuid|cr[0-4]\b|lapic|apic_id'

# limine.rs mirrors the bootloader's per-processor structures (chosen with
# #[cfg(target_arch)]), so it may name those fields and nothing else.
found=$(code | grep -E "$patterns" |
    grep -v -E '^kernel/src/limine\.rs:[0-9]+:.*(#\[cfg\((not\()?target_arch|lapic_id)' || true)
if [ -n "$found" ]; then
    echo "wall-check: processor-specific code outside kernel/src/arch/:"
    echo "$found"
    exit 1
fi
echo "wall-check: ok, no processor-specific code outside kernel/src/arch/"
