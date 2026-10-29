#!/usr/bin/env bash
# Tests for scripts/issue.sh and scripts/issue-guard.sh against a throwaway bare remote, with no GitHub
# calls. Run: bash scripts/issue-test.sh
set -uo pipefail

SCRIPT=$(cd "$(dirname "$0")" && pwd)/issue.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
export CLAIM_NO_GH=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset CLAIM_AGENT_ID

fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
# expect <code> <name> <cmd...>: run cmd, check its exit code
expect() {
  local want=$1 name=$2 got; shift 2
  "$@" >"$TMP/out" 2>&1; got=$?
  if [ "$got" = "$want" ]; then ok "$name"; else bad "$name (exit $got, wanted $want)"; sed 's/^/     /' "$TMP/out"; fi
}

git init -q --bare -b master "$TMP/remote.git"
git clone -q "$TMP/remote.git" "$TMP/repo" 2>/dev/null
cd "$TMP/repo"
git commit -q --allow-empty -m init && git push -q origin master
REPO=$TMP/repo
WT=$TMP/repo-7

expect 0 "claim makes the worktree" bash "$SCRIPT" claim 7
[ -d "$WT" ] && [ "$(git -C "$WT" branch --show-current)" = issue-7 ] \
  && ok "worktree is on issue-7" || bad "worktree is on issue-7"

# A second session in the same checkout: its claim loses, and it can't act
# on the first session's claim from the main checkout.
expect 2 "second claim exits 2" bash "$SCRIPT" claim 7
grep -q "don't touch" "$TMP/out" && ok "exit 2 names the worktree to leave alone" || bad "exit 2 names the worktree"
expect 1 "renew from the main checkout is refused" bash "$SCRIPT" renew 7
expect 1 "release from the main checkout is refused" bash "$SCRIPT" release 7
expect 1 "land from the main checkout is refused" bash "$SCRIPT" land 7

cd "$WT"
expect 0 "renew from the worktree" bash "$SCRIPT" renew 7
expect 1 "a different CLAIM_AGENT_ID is refused" env CLAIM_AGENT_ID=someone-else bash "$SCRIPT" renew 7
agent=$(sed -n 1p "$REPO/.git/agent-claims/issue-7")
expect 0 "the claiming CLAIM_AGENT_ID is accepted" env CLAIM_AGENT_ID="$agent" bash "$SCRIPT" renew 7

echo x >f && git add f && git commit -q -m "do it

Closes #7"
expect 0 "land from the worktree" bash "$SCRIPT" land 7 --verify true
[ "$(git ls-remote origin refs/heads/claim/issue-7)" = "" ] && ok "land released the claim" || bad "land released the claim"
[ "$(git rev-parse origin/master)" = "$(git rev-parse HEAD)" ] && ok "master moved to the work" || bad "master moved"

cd "$REPO"
git merge -q --ff-only origin/master
git worktree remove "$WT" && git branch -q -D issue-7

# A leftover worktree or branch stops a claim before it's pushed.
mkdir "$TMP/repo-8"
expect 1 "claim refuses an existing worktree path" bash "$SCRIPT" claim 8
[ "$(git ls-remote origin refs/heads/claim/issue-8)" = "" ] && ok "and claims nothing" || bad "and claims nothing"
rmdir "$TMP/repo-8"
git branch -q issue-8
expect 1 "claim refuses an existing issue branch" bash "$SCRIPT" claim 8
git branch -q -D issue-8

# Pushed work in progress is where a new claim's worktree starts.
git push -q origin master:refs/heads/issue-9
git commit -q --allow-empty -m "master moves on" && git push -q origin master
expect 0 "claim of an issue with work in progress" bash "$SCRIPT" claim 9
[ "$(git -C "$TMP/repo-9" rev-parse HEAD)" = "$(git rev-parse origin/issue-9)" ] \
  && ok "worktree starts from origin/issue-9" || bad "worktree starts from origin/issue-9"
(cd "$TMP/repo-9" && bash "$SCRIPT" release 9 >/dev/null) && ok "release from the worktree" || bad "release from the worktree"

# --no-worktree keeps the old behaviour: no worktree, act from anywhere.
expect 0 "claim --no-worktree" bash "$SCRIPT" claim 10 --no-worktree
[ ! -e "$TMP/repo-10" ] && ok "makes no worktree" || bad "makes no worktree"
expect 0 "release a --no-worktree claim from the main checkout" bash "$SCRIPT" release 10

# A young claim can't be stolen, except by an override with a reason (#140).
CLAIM_AGENT_ID=holder bash "$SCRIPT" claim 11 --no-worktree >/dev/null
expect 1 "steal of a young claim is refused" bash "$SCRIPT" claim 11 --steal --no-worktree
grep -q -- "--override" "$TMP/out" && ok "the refusal points at --override" || bad "the refusal points at --override"
expect 1 "--override without a reason is refused" bash "$SCRIPT" claim 11 --steal --override --no-worktree
expect 1 "--override with a blank reason is refused" bash "$SCRIPT" claim 11 --steal --override "  " --no-worktree
expect 1 "--override without --steal is refused" bash "$SCRIPT" claim 11 --override "gone" --no-worktree
expect 0 "--steal --override takes a young claim" \
  env CLAIM_AGENT_ID=taker bash "$SCRIPT" claim 11 --steal --override "Chris closed that session" --no-worktree
grep -q "by override: Chris closed that session" "$TMP/out" && ok "the output gives the reason" || bad "the output gives the reason"
body=$(git log -1 --format=%B "$(git ls-remote origin refs/heads/claim/issue-11 | cut -f1)")
grep -q "^agent: taker$" <<<"$body" && grep -q "^stolen-from: holder$" <<<"$body" \
  && grep -q "^override: Chris closed that session$" <<<"$body" \
  && ok "the claim commit records holder and reason" || bad "the claim commit records holder and reason"
expect 0 "the new holder can release it" env CLAIM_AGENT_ID=taker bash "$SCRIPT" release 11

# --here claims for the worktree it's run from (one the desktop app made).
git worktree add -q "$TMP/app-wt" -b claude/some-name master
cd "$TMP/app-wt"
expect 0 "claim --here" bash "$SCRIPT" claim 12 --here
[ "$(sed -n 2p "$REPO/.git/agent-claims/issue-12")" = "$(git rev-parse --show-toplevel)" ] \
  && ok "the record names this worktree" || bad "the record names this worktree"
expect 0 "mine from that worktree" bash "$SCRIPT" mine 12
expect 0 "mine --offline from that worktree" bash "$SCRIPT" mine 12 --offline
cd "$REPO"
expect 1 "mine from the main checkout is refused" bash "$SCRIPT" mine 12 --offline
expect 1 "mine of an unclaimed issue is refused" bash "$SCRIPT" mine 13 --offline

# The guard: a commit or push of issue work this session hasn't claimed is denied.
GUARD=$(dirname "$SCRIPT")/issue-guard.sh
mkdir -p "$REPO/scripts" "$TMP/app-wt/scripts"
cp "$SCRIPT" "$REPO/scripts/issue.sh"; cp "$SCRIPT" "$TMP/app-wt/scripts/issue.sh"
hook() { # <cwd> <command>: the JSON Claude Code sends a PreToolUse hook
  local c=${2//\\/\\\\}; c=${c//\"/\\\"}; c=${c//$'\n'/\\n}
  printf '{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"%s","tool_input":{"command":"%s"}}' "$1" "$c" | bash "$GUARD"
}
expect 0 "guard: an unrelated command passes" hook "$REPO" "ls -la"
expect 0 "guard: a commit with no issue passes" hook "$REPO" 'git commit -m "fix a typo"'
expect 2 "guard: Closes #13, unclaimed, is denied" hook "$REPO" 'git commit -m "$(cat <<'"'"'EOF'"'"'
do it

Closes #13
EOF
)"'
grep -q "scripts/issue.sh claim 13" "$TMP/out" && ok "the denial says how to claim" || bad "the denial says how to claim"
expect 0 "guard: Closes #12 from its worktree passes" hook "$TMP/app-wt" 'git commit -m "x

Fixes #12"'
expect 2 "guard: Closes #12 from the main checkout is denied" hook "$REPO" 'git commit -m "Closes #12"'
expect 0 "guard: cd into the claiming worktree first passes" hook "$REPO" "cd \"$TMP/app-wt\" && git commit -m 'Closes #12'"
git worktree add -q "$TMP/repo-14" -b issue-14 master
mkdir -p "$TMP/repo-14/scripts" && cp "$SCRIPT" "$TMP/repo-14/scripts/issue.sh"
expect 2 "guard: a commit on an unclaimed issue-14 branch is denied" hook "$TMP/repo-14" 'git commit -m "wip"'
(cd "$TMP/app-wt" && echo y >g && git add g && git commit -q -m "Closes #12")
expect 0 "guard: push of claimed work passes" hook "$TMP/app-wt" "git push -u origin HEAD"
expect 2 "guard: push from a branch with someone else's Closes #13 is denied" \
  bash -c "cd '$TMP/app-wt' && git commit -q --allow-empty -m 'Closes #13' && $(declare -f hook); GUARD='$GUARD' hook '$TMP/app-wt' 'git push'"
(cd "$TMP/app-wt" && git reset -q --hard HEAD~1)
expect 0 "guard: deleting a branch passes" hook "$REPO" "git push origin --delete issue-14"
git worktree remove -f "$TMP/repo-14"; git branch -q -D issue-14

# Once released, the worktree's push is denied: the claim is gone from origin.
(cd "$TMP/app-wt" && bash "$SCRIPT" release 12 >/dev/null)
expect 2 "guard: push after release is denied" hook "$TMP/app-wt" "git push"

# pr mode: land pushes the branch and keeps the claim.
cd "$TMP/app-wt"
bash "$SCRIPT" claim 15 --here >/dev/null
git commit -q --allow-empty -m "Closes #15"
expect 0 "land in pr mode" env CLAIM_LAND=pr bash "$SCRIPT" land 15 --verify true
[ -n "$(git ls-remote origin refs/heads/claude/some-name)" ] && ok "pr mode pushed the branch" || bad "pr mode pushed the branch"
[ -n "$(git ls-remote origin refs/heads/claim/issue-15)" ] && ok "pr mode keeps the claim" || bad "pr mode keeps the claim"
[ "$(git rev-parse origin/master)" != "$(git rev-parse HEAD)" ] && ok "pr mode leaves master alone" || bad "pr mode leaves master alone"
bash "$SCRIPT" release 15 >/dev/null
cd "$REPO"

# accept_issue_branch: an issue-<n> branch on origin (an older claim.py claim)
# holds the issue, and the session on that branch passes the guard.
echo "accept_issue_branch=1" >"$REPO/scripts/issue.conf"
git add scripts && git commit -q -m "conf" && git push -q origin master
git push -q origin master:refs/heads/issue-16
expect 2 "claim of an issue-branch-claimed issue exits 2" bash "$SCRIPT" claim 16 --no-worktree
bash "$SCRIPT" list >"$TMP/out"; grep -q "^#16 .*branch issue-16" "$TMP/out" && ok "list shows the issue-branch claim" || bad "list shows the issue-branch claim"
git worktree add -q "$TMP/repo-16" -b issue-16 origin/issue-16
expect 0 "guard: the issue-16 branch's own push passes" hook "$TMP/repo-16" "git push origin issue-16"
expect 2 "guard: an issue-17 branch without origin/issue-17 is denied" \
  bash -c "git -C '$REPO' worktree add -q '$TMP/repo-17' -b issue-17 master && $(declare -f hook); GUARD='$GUARD' hook '$TMP/repo-17' 'git push -u origin issue-17'"

echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
