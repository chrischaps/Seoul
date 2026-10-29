---
name: next-issue
description: Pick up and implement the next open GitHub issue for this repo, or a named one ("work on #N"). Use when asked to "take the next issue", "do the next one", "tackle #N" or "pick something up". Lists open issues and the claims every machine holds, skips claimed and blocked ones, claims the pick with scripts/issue.sh before reading further, works in its own worktree, verifies, then lands it.
---

# next-issue

Agents on several machines share this repo and one GitHub account, so every issue is claimed before work starts (see "Claiming issues" in the root `CLAUDE.md`). A hook refuses commits and pushes of issue work you haven't claimed.

If the user names an issue number, skip step 1, but still claim it.

## 1. Pick an issue

```bash
gh issue list --state open --limit 200 --json number,title,labels \
  --jq 'sort_by(.number)[] | "#\(.number) [\([.labels[].name] | join(","))] \(.title)"'
scripts/issue.sh list            # claims held on every machine
gh pr list --state open --json number,title,body --jq '.[] | "\(.number) \(.title)"'
```

Skip an issue if:
- **it's claimed**: it's in `scripts/issue.sh list`, or an open PR closes it;
- **it's blocked**: an issue it depends on is still open;
- **it waits on Chris**: it's labelled `needs-decision`, or its text says it needs his decision. Write up the options as a comment if nobody has, and stop.

Of what's left, take bugs that hurt first, then the lowest number. Chris can always name an issue or an area; that wins. Tell him which you picked and why lower ones were skipped, then go on without waiting.

## 2. Claim it, before reading anything else

```bash
scripts/issue.sh claim <n>          # makes ../<repo>-<n> on branch issue-<n>
scripts/issue.sh claim <n> --here   # or: claim for the worktree this session is already in
```

Exit 2 means someone else holds it, perhaps seconds ago on another machine. Go back to the list and take the next one. Never touch a worktree for an issue you didn't claim. If Chris tells you to take over a claim he knows is dead: `scripts/issue.sh claim <n> --steal --override "<his reason>"`.

## 3. Work

- `gh issue view <n> --comments`: read the whole issue, its acceptance list, and the issues it links.
- Check Claude's Vault for recent notes on this project.
- Work only in the claimed worktree. Stage files by path, never `git add -A`. Leave the main checkout and other sessions' changes alone.
- Work lasting more than a day: `scripts/issue.sh renew <n>` at the start of each session, and push work in progress (`git push origin HEAD`) so it survives this machine.

## 4. Verify

Run the issue's own Verify steps and the tests for what you changed. Tick off each acceptance item honestly; mark anything only a human can judge with **[H]**.

## 5. Land

Commit with `Closes #<n>` in the message, then:

```bash
scripts/issue.sh land <n>
```

`land` runs the repo's verify command from `scripts/issue.conf`. In push mode it catches up with the base branch, pushes fast-forward only and releases the claim; exit 3 is a conflict to resolve before landing again. In pr mode it pushes the branch and opens a PR; the claim stays until the PR merges, then `scripts/issue.sh release <n>`.

Post a closing comment on the issue saying what was built and how it was verified. Then remove your worktree from the main checkout (`git worktree remove ../<repo>-<n>`).

## 6. Present and leave a trail

Make a short presentation Artifact that shows the change with real evidence, link it in the closing comment, and write a Vault note for the next agent.
