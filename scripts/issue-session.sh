#!/usr/bin/env bash
# SessionStart hook: tells a new session that this repo claims issues before
# work, who holds what right now, and which claims this checkout made.
# Whatever it prints becomes context for the session. It never fails a start.
#
# The canonical copy is skills/setup-claims in chrischaps/Dashboard.

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
top=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -f "$top/scripts/issue.sh" ] || exit 0

claims=$(timeout 20 bash "$top/scripts/issue.sh" list 2>/dev/null) || claims="(couldn't reach origin to list claims)"

mine=""
dir="$(git rev-parse --path-format=absolute --git-common-dir)/agent-claims"
for rec in "$dir"/issue-*; do
  [ -f "$rec" ] || continue
  wt=$(sed -n 2p "$rec")
  mine+="  #${rec##*/issue-} as $(sed -n 1p "$rec")${wt:+, worktree $wt}
"
done

cat <<EOF
This repo claims GitHub issues before working on them, because agents on several machines share one GitHub account (see "Claiming issues" in CLAUDE.md).
- Before any work on issue #n: scripts/issue.sh claim <n> (new worktree), or claim <n> --here (this worktree). Exit 2 = someone else holds it; pick another.
- A hook refuses git commit / git push of issue work (an issue-<n> branch, or "Closes #n") that this session hasn't claimed.
- Long work: scripts/issue.sh renew <n> each session (claims go stale after 24h). Done: scripts/issue.sh land <n>.
Claims on origin now:
$(sed 's/^/  /' <<<"$claims")
EOF
[ -z "$mine" ] || printf 'Claimed from this checkout (only the session working in that worktree may use them):\n%s' "$mine"
exit 0
