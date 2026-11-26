#!/usr/bin/env bash
# Prints one version's section of CHANGELOG.md (without its heading): the body of its GitHub release.
#   scripts/changelog-section.sh 0.1.0
set -euo pipefail
version=$1
awk -v v="$version" '
  /^## / { if (found) exit; if (index($0, "## v" v " ") == 1 || index($0, "## " v " ") == 1) { found = 1; next } }
  found { print }
' "$(dirname "$0")/../CHANGELOG.md" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
