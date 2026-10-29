#!/usr/bin/env bash
# Claim, renew, release and land GitHub issues for agents that share one
# GitHub account across several machines.
#
# A claim is a branch on origin, claim/issue-<n>, holding one commit with no
# files whose message names the agent. Git updates a remote ref atomically,
# so when two agents push a claim for the same issue, the second push is
# rejected as non-fast-forward and only one agent wins. Labels, assignees and
# comments can't do that: every agent is the same GitHub user, and none of
# them is compare-and-swap.
#
#   scripts/issue.sh list                     every claim: issue, holder, age
#   scripts/issue.sh claim <n>                claim issue n (exit 2 if held) and
#                                             make its worktree, ../<repo>-<n>
#   scripts/issue.sh claim <n> --here         claim it for the worktree you're in
#                                             (one the desktop app made, say)
#   scripts/issue.sh claim <n> --steal        take over a claim older than the TTL
#   scripts/issue.sh claim <n> --steal --override "<reason>"
#                                             take over a younger claim, because
#                                             a human asked; the reason is recorded
#   scripts/issue.sh claim <n> --no-worktree  claim without making a worktree
#   scripts/issue.sh renew <n>                refresh your claim's timestamp
#   scripts/issue.sh release <n>              give up your claim
#   scripts/issue.sh land <n> [--verify CMD]  verify, then deliver: push mode
#                                             catches up with the base branch,
#                                             pushes to it and releases; pr mode
#                                             pushes the branch and opens a PR
#   scripts/issue.sh mine <n> [--offline]     exit 0 if you hold n here (hooks)
#
# --override is for a human who knows the holder is gone (a session they
# closed, a machine that's off). Agents use it only when asked to. The reason
# goes into the claim commit and the issue comment, so the takeover is on
# record.
#
# Two sessions started in one checkout share its .git, so the record of a
# claim can't by itself say which of them made it. So claim makes the
# worktree (or, with --here, takes the one it's run from), records its path
# beside the agent id, and renew, release and land run only from that
# worktree. A session whose claim exits 2 gets no worktree, and must leave
# the existing one alone.
#
# Per repo settings live in scripts/issue.conf, a bash file of plain
# assignments, all optional:
#   base=main                   branch work lands on (default: origin's HEAD)
#   land=pr                     push: fast-forward the base branch; pr: open a PR
#   verify="scripts/ci.sh"      land's --verify when none is given
#   accept_issue_branch=1       an issue-<n> branch on origin also counts as a
#                               claim, for repos that claimed that way before
#
# Environment (wins over issue.conf):
#   CLAIM_REMOTE     remote to use (default origin)
#   CLAIM_BASE, CLAIM_LAND, CLAIM_VERIFY   as base, land and verify above
#   CLAIM_TTL_HOURS  age after which a claim may be stolen (default 24)
#   CLAIM_AGENT_ID   name to claim under (default <host>-<time>-<random>);
#                    when set, renew, release and land refuse a claim made
#                    under another name
#   CLAIM_NO_GH=1    skip GitHub calls (labels, comments, issue state, PRs); for tests
#
# The canonical copy is skills/setup-claims in chrischaps/Dashboard; each repo
# vendors it. Change it there and re-run its install.sh, not here.

set -euo pipefail

CONF="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/issue.conf"
base="" land="" verify="" accept_issue_branch=""
# shellcheck disable=SC1090
[ -f "$CONF" ] && . "$CONF"

REMOTE=${CLAIM_REMOTE:-origin}
TTL_HOURS=${CLAIM_TTL_HOURS:-24}
LAND_MODE=${CLAIM_LAND:-${land:-push}}
DEFAULT_VERIFY=${CLAIM_VERIFY:-${verify:-}}
ISSUE_BRANCH_CLAIMS=${accept_issue_branch:-}
LABEL=claimed

die() { echo "issue.sh: $*" >&2; exit 1; }

usage() { sed -n '12,28p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

gh_on() { [ "${CLAIM_NO_GH:-}" != 1 ]; }

# The branch work lands on: issue.conf, else origin's HEAD, else main or master.
base_branch() {
  local b=${CLAIM_BASE:-$base}
  if [ -z "$b" ]; then
    b=$(git symbolic-ref -q --short "refs/remotes/$REMOTE/HEAD" 2>/dev/null || true)
    b=${b#"$REMOTE"/}
  fi
  if [ -z "$b" ] && git rev-parse -q --verify "$REMOTE/main" >/dev/null; then b=main; fi
  echo "${b:-master}"
}

ref_of() { echo "refs/heads/claim/issue-$1"; }

# Local record of the claims this checkout made, shared by all its worktrees.
# Line 1 is the agent id, line 2 the worktree the claim is for (absent after
# --no-worktree). Only the machine that claimed an issue can renew, release
# or land it, and only from that worktree.
common_dir() { git rev-parse --path-format=absolute --git-common-dir; }
record_dir() { echo "$(common_dir)/agent-claims"; }
record_of() { echo "$(record_dir)/issue-$1"; }
record_agent() { sed -n 1p "$(record_of "$1")"; }
record_worktree() { sed -n 2p "$(record_of "$1")"; }

# ../<repo>-<n> beside the main checkout, whichever worktree we're run from.
worktree_of() {
  local main
  main=$(dirname "$(common_dir)")
  echo "$(dirname "$main")/$(basename "$main")-$1"
}

now() { date +%s; }

# Prints "<sha> <committer time>" of the remote claim, or nothing if unclaimed.
remote_claim() {
  local ref sha
  ref=$(ref_of "$1")
  sha=$(git ls-remote "$REMOTE" "$ref" | cut -f1)
  [ -n "$sha" ] || return 0
  git fetch -q "$REMOTE" "$ref" 2>/dev/null || true
  echo "$sha $(git log -1 --format=%ct "$sha")"
}

# The sha of origin's issue-<n> branch, when such branches count as claims.
issue_branch_claim() {
  [ "$ISSUE_BRANCH_CLAIMS" = 1 ] || return 0
  git ls-remote "$REMOTE" "refs/heads/issue-$1" | cut -f1
}

claim_body() { git log -1 --format=%B "$1"; }

agent_of() { claim_body "$1" | sed -n 's/^agent: //p'; }

age_text() {
  local s=$(( $(now) - $1 ))
  if [ "$s" -lt 3600 ]; then echo "$(( s / 60 ))m"; else echo "$(( s / 3600 ))h$(( s % 3600 / 60 ))m"; fi
}

make_claim() { # <n> <agent> [parent] [extra trailer lines]
  local msg parent=()
  [ -n "${3:-}" ] && parent=(-p "$3")
  msg="claim #$1

agent: $2
host: $(hostname)
at: $(date -u +%Y-%m-%dT%H:%M:%SZ)${4:+
$4}"
  printf '%s\n' "$msg" | git commit-tree "$(git mktree </dev/null)" ${parent[@]+"${parent[@]}"}
}

# Why this checkout doesn't hold n here, or nothing if it does. Local checks
# only: the record exists, the agent id matches, and we're in its worktree.
not_mine_locally() {
  local n=$1 rec agent wt here
  rec=$(record_of "$n")
  [ -f "$rec" ] || { echo "#$n wasn't claimed from this checkout (no $rec)"; return; }
  agent=$(record_agent "$n")
  if [ -n "${CLAIM_AGENT_ID:-}" ] && [ "$CLAIM_AGENT_ID" != "$agent" ]; then
    echo "#$n was claimed here as $agent, not as $CLAIM_AGENT_ID: it's another session's claim"
    return
  fi
  wt=$(record_worktree "$n")
  if [ -n "$wt" ]; then
    here=$(git rev-parse --show-toplevel)
    [ "$here" = "$wt" ] \
      || echo "#$n belongs to the session working in $wt; run this from there, not $here. If that session isn't you, leave it alone"
  fi
}

# Fails unless this checkout holds the claim and we're in the worktree that
# claim is for. Prints the remote claim sha.
require_mine() {
  local n=$1 why sha _t agent
  why=$(not_mine_locally "$n")
  [ -z "$why" ] || die "$why"
  agent=$(record_agent "$n")
  read -r sha _t <<<"$(remote_claim "$n")"
  [ -n "${sha:-}" ] || die "#$n has no claim on $REMOTE"
  [ "$(agent_of "$sha")" = "$agent" ] \
    || die "#$n is now held by $(agent_of "$sha"), not $agent"
  echo "$sha"
}

# mine <n> [--offline]: exit 0 if this session holds n here, else print why
# and exit 1. With accept_issue_branch, being on issue-<n> with origin/issue-<n>
# pushed counts too. --offline skips origin, so it's quick but only local.
cmd_mine() {
  local n=${1:?issue number} offline="" why sha _t
  [ "${2:-}" = --offline ] && offline=1
  why=$(not_mine_locally "$n")
  if [ -z "$why" ]; then
    [ -n "$offline" ] && return 0
    read -r sha _t <<<"$(remote_claim "$n")"
    if [ -z "${sha:-}" ]; then why="#$n has no claim on $REMOTE (released, or never pushed)"
    elif [ "$(agent_of "$sha")" != "$(record_agent "$n")" ]; then why="#$n is now held by $(agent_of "$sha")"
    else return 0
    fi
  fi
  if [ "$ISSUE_BRANCH_CLAIMS" = 1 ] && [ "$(git branch --show-current)" = "issue-$n" ]; then
    { [ -n "$offline" ] && git show-ref -q --verify "refs/remotes/$REMOTE/issue-$n"; } && return 0
    [ -n "$(issue_branch_claim "$n")" ] && return 0
  fi
  echo "$why"
  return 1
}

cmd_list() {
  local found=0 ref sha n
  while read -r sha ref; do
    [ -n "$sha" ] || continue
    found=1
    n=${ref##*/issue-}
    git fetch -q "$REMOTE" "$ref" 2>/dev/null || true
    printf '#%-5s %-40s %s ago\n' "$n" "$(agent_of "$sha")" "$(age_text "$(git log -1 --format=%ct "$sha")")"
  done < <(git ls-remote "$REMOTE" 'refs/heads/claim/issue-*')
  if [ "$ISSUE_BRANCH_CLAIMS" = 1 ]; then
    while read -r sha ref; do
      [ -n "$sha" ] || continue
      found=1
      printf '#%-5s %-40s no TTL\n' "${ref##*/issue-}" "branch issue-${ref##*/issue-}"
    done < <(git ls-remote "$REMOTE" 'refs/heads/issue-*')
  fi
  [ "$found" = 1 ] || echo "no claims"
}

held() { # <n> <holder> <claim time or empty> <worktree>
  echo "#$1 is already claimed by $2${3:+, $(age_text "$3") ago}. It isn't yours: pick another issue, and don't touch $4" >&2
  exit 2
}

cmd_claim() {
  local n=${1:?issue number} steal="" override="" make_wt=1 here="" ref agent sha old old_t state wt base_ref="" note="" b
  shift
  while [ $# -gt 0 ]; do
    case $1 in
      --steal) steal=--steal ;;
      --override)
        [ $# -ge 2 ] && [ -n "${2// /}" ] && [ "${2#-}" = "$2" ] || die "--override needs a reason, e.g. --override \"Chris closed that session\""
        override=$2; shift ;;
      --no-worktree) make_wt="" ;;
      --here) make_wt="" here=1 ;;
      *) die "claim: unknown option $1" ;;
    esac
    shift
  done
  [ -z "$override" ] || [ -n "$steal" ] || die "--override goes with --steal"
  ref=$(ref_of "$n")
  if gh_on; then
    state=$(gh issue view "$n" --json state -q .state)
    [ "$state" = OPEN ] || die "#$n is $state"
  fi

  # A held claim is exit 2 even when its worktree is here, so the session
  # that lost knows to pick another issue.
  wt=$(worktree_of "$n")
  [ -z "$here" ] || wt=$(git rev-parse --show-toplevel)
  if [ -z "$steal" ]; then
    read -r old old_t <<<"$(remote_claim "$n")"
    [ -z "${old:-}" ] || held "$n" "$(agent_of "$old")" "$old_t" "$wt"
    if [ -n "$(issue_branch_claim "$n")" ] && [ "$(git branch --show-current)" != "issue-$n" ]; then
      held "$n" "the branch issue-$n on $REMOTE (an older-style claim)" "" "$wt"
    fi
  fi

  # Checked before claiming, so a refusal leaves nothing to undo. A worktree
  # or branch already here is another session's, or a stale one: never adopt it.
  if [ -n "$make_wt" ]; then
    [ ! -e "$wt" ] \
      || die "$wt already exists: another session's worktree, or a stale one. Leave it alone unless it's stale ('issue.sh list', 'git -C $wt status'); then push or drop its work, remove it, and claim again"
    ! git show-ref -q --verify "refs/heads/issue-$n" \
      || die "branch issue-$n already exists here. If it's stale, push or drop its work, delete it, and claim again"
  fi
  if [ -n "$here" ] && [ -f "$(record_of "$n")" ]; then
    die "this checkout already has a record for #$n ($(record_of "$n")): another session's claim, or a stale one"
  fi

  agent=${CLAIM_AGENT_ID:-$(hostname)-$(now)-$RANDOM}

  if [ -n "$steal" ]; then
    read -r old old_t <<<"$(remote_claim "$n")"
    [ -n "${old:-}" ] || die "#$n isn't claimed; claim it without --steal"
    if [ -n "$override" ]; then
      note="taken over from \`$(agent_of "$old")\`'s $(age_text "$old_t")-old claim, by override: $override"
      sha=$(make_claim "$n" "$agent" "" "stolen-from: $(agent_of "$old")
override: $override")
    else
      [ $(( $(now) - old_t )) -ge $(( TTL_HOURS * 3600 )) ] \
        || die "#$n claim by $(agent_of "$old") is $(age_text "$old_t") old, under the ${TTL_HOURS}h TTL. Only a human can say it's gone sooner: then add --override \"<reason>\""
      note="taken over from \`$(agent_of "$old")\`'s stale claim"
      sha=$(make_claim "$n" "$agent" "" "stolen-from: $(agent_of "$old")")
    fi
    # Compare-and-swap: fails if the holder renewed since we looked.
    git push -q --force-with-lease="$ref:$old" "$REMOTE" "$sha:$ref" \
      || die "#$n claim changed while stealing it; run list and look again"
  else
    sha=$(make_claim "$n" "$agent")
    if ! git push -q "$REMOTE" "$sha:$ref" 2>/dev/null; then
      read -r old old_t <<<"$(remote_claim "$n")"
      [ -n "${old:-}" ] || die "push of the claim for #$n failed, and nobody holds it; check the network and retry"
      held "$n" "$(agent_of "$old")" "$old_t" "$wt"
    fi
  fi

  mkdir -p "$(record_dir)"
  if [ -n "$make_wt" ]; then
    # Continue an earlier agent's pushed work in progress, if there is any.
    b=$(base_branch)
    git fetch -q "$REMOTE" "$b"
    base_ref=$REMOTE/$b
    if git fetch -q "$REMOTE" "refs/heads/issue-$n:refs/remotes/$REMOTE/issue-$n" 2>/dev/null; then
      base_ref=$REMOTE/issue-$n
    fi
    if ! git worktree add -q "$wt" -b "issue-$n" "$base_ref"; then
      git push -q --force-with-lease="$ref:$sha" "$REMOTE" ":$ref" || true
      die "couldn't make the worktree $wt, so the claim on #$n was dropped"
    fi
    wt=$(git -C "$wt" rev-parse --show-toplevel)
    printf '%s\n%s\n' "$agent" "$wt" >"$(record_of "$n")"
  elif [ -n "$here" ]; then
    printf '%s\n%s\n' "$agent" "$wt" >"$(record_of "$n")"
  else
    echo "$agent" >"$(record_of "$n")"
  fi
  if gh_on; then
    gh issue edit "$n" --add-label "$LABEL" >/dev/null 2>&1 \
      || { gh label create "$LABEL" --color FBCA04 --description "an agent holds claim/issue-<n>" >/dev/null 2>&1 \
           && gh issue edit "$n" --add-label "$LABEL" >/dev/null 2>&1; } || true
    gh issue comment "$n" --body "Claimed by \`$agent\` on \`$(hostname)\`${note:+, $note}." >/dev/null || true
  fi
  echo "claimed #$n as $agent${note:+, ${note//\`/}}"
  if [ -n "$make_wt" ]; then
    echo "worktree: $wt (branch issue-$n from $base_ref). Work there; renew, release and land run only from it"
  elif [ -n "$here" ]; then
    echo "for this worktree: $wt. Renew, release and land run only from it"
  fi
}

cmd_renew() {
  local n=${1:?issue number} old new
  old=$(require_mine "$n")
  new=$(make_claim "$n" "$(record_agent "$n")" "$old")
  git push -q "$REMOTE" "$new:$(ref_of "$n")" || die "renew of #$n was rejected"
  echo "renewed #$n"
}

cmd_release() {
  local n=${1:?issue number} sha
  sha=$(require_mine "$n")
  git push -q --force-with-lease="$(ref_of "$n"):$sha" "$REMOTE" ":$(ref_of "$n")" \
    || die "release of #$n was rejected"
  rm -f "$(record_of "$n")"
  gh_on && { gh issue edit "$n" --remove-label "$LABEL" >/dev/null 2>&1 || true; }
  echo "released #$n"
}

run_verify() { echo "verifying: $1"; bash -c "$1" || die "verify failed; fix it, commit, land again"; }

cmd_land() {
  local n=${1:?issue number} verify="" attempt b branch
  shift
  if [ "${1:-}" = --verify ]; then verify=${2:?--verify needs a command}; else verify=$DEFAULT_VERIFY; fi
  require_mine "$n" >/dev/null
  git diff --quiet && git diff --cached --quiet || die "commit or drop your changes first"
  b=$(base_branch)
  git fetch -q "$REMOTE" "$b"
  git log "$REMOTE/$b..HEAD" --format=%B | grep -qiE "(close[sd]?|fix(e[sd])?|resolve[sd]?) #$n\b" \
    || echo "issue.sh: warning: no commit says 'Closes #$n'" >&2

  if [ "$LAND_MODE" = pr ]; then
    # The branch goes up for review; the claim stays until the PR merges
    # (release it then), so nobody else picks the issue up meanwhile.
    branch=$(git branch --show-current)
    [ -n "$branch" ] && [ "$branch" != "$b" ] || die "land in pr mode needs a work branch, not $b"
    [ -z "$verify" ] || run_verify "$verify"
    git push -q -u "$REMOTE" "HEAD:refs/heads/$branch" || die "push of $branch was rejected; fetch, catch up with $REMOTE/$branch, land again"
    if gh_on; then
      if gh pr view "$branch" --json url -q .url >/dev/null 2>&1; then
        echo "pushed $branch; its PR: $(gh pr view "$branch" --json url -q .url)"
      else
        gh pr create --base "$b" --head "$branch" --fill >/dev/null || die "pushed $branch, but couldn't open a PR; open it by hand"
        echo "opened $(gh pr view "$branch" --json url -q .url)"
      fi
    else
      echo "pushed $branch"
    fi
    echo "the claim on #$n stays until the PR merges; then run: scripts/issue.sh release $n"
    return 0
  fi

  # The commit --verify last passed on. Verify runs before the first push and
  # again whenever catching up with the base branch changes HEAD, so a land
  # after a conflict resolved by hand is still verified.
  local verified=""
  for attempt in 1 2 3 4 5; do
    if ! git merge-base --is-ancestor "$REMOTE/$b" HEAD; then
      # A rebase flattens merges, and a subtree pull's merge is what the next
      # `git subtree pull`/`push` finds its base by. So a branch holding any
      # merge, or a squashed subtree commit, merges the base branch in instead.
      if [ -n "$(git rev-list --merges "$REMOTE/$b..HEAD")" ] \
        || git log "$REMOTE/$b..HEAD" --format=%B | grep -q '^git-subtree-dir:'; then
        echo "$b moved; merging it in to keep the branch's merges (attempt $attempt)"
        git merge -q --no-edit "$REMOTE/$b" || {
          echo "issue.sh: merge conflict. Resolve it, 'git commit', then land again." >&2
          exit 3
        }
      else
        echo "$b moved; rebasing (attempt $attempt)"
        git rebase -q "$REMOTE/$b" || {
          echo "issue.sh: rebase conflict. Resolve it, 'git rebase --continue', then land again." >&2
          exit 3
        }
      fi
    fi
    if [ -n "$verify" ] && [ "$verified" != "$(git rev-parse HEAD)" ]; then
      run_verify "$verify"
      verified=$(git rev-parse HEAD)
    fi
    # Fast-forward only: a rejection means someone landed first, so loop.
    if git push -q "$REMOTE" "HEAD:$b"; then
      echo "landed #$n at $(git rev-parse --short HEAD)"
      cmd_release "$n"
      git push -q "$REMOTE" --delete "issue-$n" 2>/dev/null || true
      return 0
    fi
    git fetch -q "$REMOTE" "$b"
  done
  die "$b kept moving; land again"
}

case ${1:-} in
  list) cmd_list ;;
  claim) shift; cmd_claim "$@" ;;
  renew) shift; cmd_renew "$@" ;;
  release) shift; cmd_release "$@" ;;
  land) shift; cmd_land "$@" ;;
  mine) shift; cmd_mine "$@" ;;
  *) usage ;;
esac
