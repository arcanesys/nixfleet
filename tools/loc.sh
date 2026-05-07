#!/usr/bin/env bash
# LOC report for the Rust workspace, splitting prod vs test code.
#
# Why custom: tokei/cloc count files, but ~25% of our Rust LOC lives in
# `#[cfg(test)] mod tests { ... }` blocks at the bottom of src/ files.
# A naive count makes the codebase look ~2x its actual size.
#
# Categories:
#   prod          src/*.rs above the first inline `#[cfg(test)]` line,
#                 excluding files marked `#![cfg(test)]` at module level
#                 (e.g. lifecycle_parity_tests.rs, gates/tests.rs).
#   inline-tests  src/*.rs from the first `#[cfg(test)]` line to EOF,
#                 plus whole files gated with `#![cfg(test)]`.
#   integ-tests   crates/*/tests/*.rs.
#
# Usage:
#   tools/loc.sh                 # human-readable report
#   tools/loc.sh --update        # rewrite tools/loc-baseline.txt
#   tools/loc.sh --check         # diff against baseline (CI-friendly)

set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

mode=${1:-report}

prod=0
inline=0
integ=0
prod_files=0
inline_files=0
integ_files=0

while IFS= read -r -d '' f; do
  total=$(wc -l <"$f")
  if [[ $f == */tests/* ]]; then
    integ=$((integ + total))
    integ_files=$((integ_files + 1))
    continue
  fi
  # Whole-file test modules: `#![cfg(test)]` at the top.
  if head -10 "$f" | grep -q '^#!\[cfg(test)\]'; then
    inline=$((inline + total))
    inline_files=$((inline_files + 1))
    continue
  fi
  split_line=$(grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1 || true)
  if [[ -n $split_line ]]; then
    prod_part=$((split_line - 1))
    test_part=$((total - split_line + 1))
    prod=$((prod + prod_part))
    inline=$((inline + test_part))
    prod_files=$((prod_files + 1))
    inline_files=$((inline_files + 1))
  else
    prod=$((prod + total))
    prod_files=$((prod_files + 1))
  fi
done < <(find crates -type f -name '*.rs' -not -path '*/target/*' -print0)

total=$((prod + inline + integ))
prod_pct=$(awk "BEGIN { printf \"%.1f\", $prod * 100 / $total }")
inline_pct=$(awk "BEGIN { printf \"%.1f\", $inline * 100 / $total }")
integ_pct=$(awk "BEGIN { printf \"%.1f\", $integ * 100 / $total }")
test_pct=$(awk "BEGIN { printf \"%.1f\", ($inline + $integ) * 100 / $total }")

emit_report() {
  local with_meta=${1:-0}
  if [[ $with_meta == 1 ]]; then
    echo "NixFleet Rust LOC report ($(git describe --always --dirty 2>/dev/null || echo unknown))"
    echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo
  fi
  cat <<EOF
  Category        LOC          %     Files
  prod            $(printf '%6d' "$prod")  $(printf '%6s' "$prod_pct")%   $(printf '%4d' "$prod_files")
  inline-tests    $(printf '%6d' "$inline")  $(printf '%6s' "$inline_pct")%   $(printf '%4d' "$inline_files")
  integ-tests     $(printf '%6d' "$integ")  $(printf '%6s' "$integ_pct")%   $(printf '%4d' "$integ_files")
  ----------------------------------------------
  total           $(printf '%6d' "$total")  100.0%

  Tests overall:  $((inline + integ)) LOC ($test_pct% of total)
EOF

  if command -v tokei >/dev/null 2>&1; then
    echo
    echo "Per-crate (tokei, file-level counts — does not split inline tests):"
    tokei --type Rust --sort lines --output table crates 2>/dev/null |
      awk '/^---/ {n++; if (n==2) exit} n==1 || /Rust/'
  elif command -v cloc >/dev/null 2>&1; then
    echo
    echo "Per-crate (cloc):"
    cloc --include-lang=Rust --quiet crates
  fi
}

case "$mode" in
report | --report) emit_report 1 ;;
--update)
  emit_report 0 >tools/loc-baseline.txt
  echo "Wrote tools/loc-baseline.txt"
  ;;
--check)
  tmp=$(mktemp)
  emit_report 0 >"$tmp"
  if ! diff -u tools/loc-baseline.txt "$tmp"; then
    echo "LOC report drifted from baseline. Run 'tools/loc.sh --update' to refresh." >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  ;;
*)
  echo "usage: $0 [--report|--update|--check]" >&2
  exit 2
  ;;
esac
