#!/usr/bin/env bash
# Picks the commit range the author check inspects for a GitHub Actions event.
#
# Kept out of the workflow so the range rules can be tested: a deleted branch has no
# commits to inspect and must not fail the job, while a brand new or rewritten history
# has to fall back to inspecting everything.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
check="$here/check-authors.sh"
zero=0000000000000000000000000000000000000000

if [[ "${EVENT_NAME:-push}" == "pull_request" ]]; then
  exec "$check" "${PR_BASE}..${PR_HEAD}"
fi

if [[ "${PUSH_AFTER:-}" == "$zero" ]]; then
  echo "ref deleted; nothing to check"
  exit 0
fi

if [[ -n "${PUSH_BEFORE:-}" && "${PUSH_BEFORE}" != "$zero" ]] \
  && git cat-file -e "${PUSH_BEFORE}^{commit}" 2>/dev/null; then
  exec "$check" "${PUSH_BEFORE}..${PUSH_AFTER}"
fi

# First push to a branch, or a rewritten history: inspect everything.
exec "$check"
