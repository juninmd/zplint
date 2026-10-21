#!/usr/bin/env bash
# Conventional-commit release helper used by .github/workflows/release.yml.
#
#   release.sh next <current-version>   NUL-separated commit messages on stdin
#                                       -> prints the next version, or "none"
#   release.sh notes <version> [range]  prints a changelog section for `range`
#                                       (default: last v* tag..HEAD)
#
# Bump rules (Conventional Commits):  feat -> minor, fix/perf -> patch,
# `type!:` or a `BREAKING CHANGE:` footer -> major (minor while the major is 0).
# Other types (docs, chore, ci, test, refactor, style, build) do not release.
set -euo pipefail

TYPES='feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert'
HEADER_RE="^(${TYPES})(\([^)]+\))?(!)?: .+"

next() {
  local current=${1:?current version}
  local bump=none msg subject
  while IFS= read -r -d '' msg; do
    subject=${msg%%$'\n'*}
    [[ $subject =~ $HEADER_RE ]] || continue
    local type=${BASH_REMATCH[1]} bang=${BASH_REMATCH[3]}
    if [[ -n $bang || $msg == *$'\n'BREAKING\ CHANGE:* || $msg == *$'\n'BREAKING-CHANGE:* ]]; then
      bump=major
    elif [[ $type == feat && $bump != major ]]; then
      bump=minor
    elif [[ $type == fix || $type == perf ]] && [[ $bump == none ]]; then
      bump=patch
    fi
  done
  [[ $bump == none ]] && { echo none; return; }

  IFS=. read -r maj min pat <<<"${current%%[-+]*}"
  if [[ $bump == major && $maj -eq 0 ]]; then bump=minor; fi
  case $bump in
    major) echo "$((maj + 1)).0.0" ;;
    minor) echo "${maj}.$((min + 1)).0" ;;
    patch) echo "${maj}.${min}.$((pat + 1))" ;;
  esac
}

notes() {
  local version=${1:?version} range=${2:-}
  if [[ -z $range ]]; then
    local last
    last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
    range=${last:+$last..}HEAD
  fi
  local -A section=([feat]="Features" [fix]="Bug Fixes" [perf]="Performance" [revert]="Reverts")
  local breaking="" subject
  declare -A body
  while IFS= read -r -d '' msg; do
    subject=${msg%%$'\n'*}
    [[ $subject =~ $HEADER_RE ]] || continue
    local type=${BASH_REMATCH[1]} scope=${BASH_REMATCH[2]} bang=${BASH_REMATCH[3]}
    local text=${subject#*: }
    [[ -n $scope ]] && text="**${scope:1:${#scope}-2}:** $text"
    if [[ -n $bang || $msg == *$'\n'BREAKING\ CHANGE:* ]]; then
      breaking+="- $text"$'\n'
    fi
    [[ -n ${section[$type]:-} ]] && body[$type]+="- $text"$'\n'
  done < <(git log --no-merges --format='%B%x00' "$range")

  printf '## [%s] - %s\n' "$version" "$(date -u +%F)"
  [[ -n $breaking ]] && printf '\n### BREAKING CHANGES\n\n%s' "$breaking"
  for t in feat fix perf revert; do
    [[ -n ${body[$t]:-} ]] && printf '\n### %s\n\n%s' "${section[$t]}" "${body[$t]}"
  done
  return 0
}

case ${1:-} in
  next) shift; next "$@" ;;
  notes) shift; notes "$@" ;;
  *) echo "usage: $0 next <current> | notes <version> [range]" >&2; exit 2 ;;
esac
