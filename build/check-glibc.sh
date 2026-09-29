#!/usr/bin/env bash
# GLIBC compatibility gate for MyCut Linux artifacts.
#
# Fails (exit 1) if ANY inspected ELF requires GLIBC newer than the target
# (default 2.31 = Ubuntu 20.04.6 LTS), or if an executable links against
# unexpected non-whitelisted system libraries (e.g. webkit2gtk slipping in).
#
# Usage: check-glibc.sh [--max 2.31] FILE [FILE...]
# Inspect EVERY shipped ELF: main binaries, sidecars, helper binaries.
set -euo pipefail

MAX="2.31"
if [ "${1:-}" = "--max" ]; then MAX="$2"; shift 2; fi
if [ $# -eq 0 ]; then
  echo "usage: $0 [--max N.NN] FILE..." >&2
  exit 2
fi

ver_lt() { # ver_lt A B -> true if A < B
  [ "$(printf '%s\n' "$1" "$2" | sort -V | head -1)" != "$2" ]
}

# Dynamic libs a MyCut ELF is allowed to need (glibc family + compiler RT
# + zlib). Anything else (webkit2gtk, gtk, ...) fails the gate.
ALLOWED='^(linux-vdso|ld-linux.*|libc\.so.*|libm\.so.*|libmvec\.so.*|libdl\.so.*|libpthread\.so.*|librt\.so.*|libresolv\.so.*|libgcc_s\.so.*|libstdc\+\+\.so.*|libz\.so.*|libcrypt\.so.*)$'

fail=0
for f in "$@"; do
  if [ ! -f "$f" ]; then echo "GLIBC-GATE: missing file: $f" >&2; fail=1; continue; fi
  if ! file -b "$f" | grep -q "ELF"; then
    echo "GLIBC-GATE: skip (not ELF): $f"
    continue
  fi
  # Highest required GLIBC symbol version (empty for static binaries).
  max_req="$(objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_2\.[0-9]*' | sort -Vu | tail -1 || true)"
  if [ -z "$max_req" ]; then
    echo "GLIBC-GATE: OK   (static) $f"
  else
    ver="${max_req#GLIBC_}"
    if ver_lt "$MAX" "$ver"; then
      echo "GLIBC-GATE: FAIL $f requires $max_req > target $MAX"
      objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_2\.[0-9]*' | sort -Vu | tail -5 | sed 's/^/    /' >&2
      fail=1
    else
      echo "GLIBC-GATE: OK   (<= $MAX) $f -> $max_req"
    fi
  fi
  # NEEDED whitelist check for dynamically linked executables.
  if file -b "$f" | grep -q "dynamically linked"; then
    bad=""
    while read -r lib; do
      [ -z "$lib" ] && continue
      echo "$lib" | grep -Eq "$ALLOWED" || bad="$bad $lib"
    done < <(objdump -p "$f" 2>/dev/null | awk '/NEEDED/ {print $2}' || true)
    if [ -n "$bad" ]; then
      echo "GLIBC-GATE: FAIL $f links unexpected libraries:$bad (allowed: glibc family, libgcc, libstdc++, libz)"
      fail=1
    fi
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "GLIBC-GATE: FAILED — artifacts exceed the $MAX target" >&2
  exit 1
fi
echo "GLIBC-GATE: PASSED (target <= $MAX)"
