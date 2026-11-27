---
name: release
description: Releases Mighty Code with scripts/release.py, the notes written by the release-notes agent and a failed CI run read by the ci-failure agent. Use when the user asks for a release, e.g. "릴리즈 해", "0.4.56 릴리즈해 줘", "release".
argument-hint: "[version]"
---

# Release

`docs/release.md` is the procedure: what every step checks and every option. This is the order this repository runs it in.

## Before anything

- On `main`, clean tree, up to date with `origin/main` (`git status`, `git fetch`).
- Windows and phone parity is done for what ships: ask the user when you cannot tell.
- The version: the one the user named, else the next patch after `VERSION`. Say which before step 1.
- The user asked for this release: that is what allows the `--yes` steps. Without it, run none of them.

## The checklist

Before step 0, make the task list with one task per step below (TaskCreate, or TodoWrite where the task tools are off), in this order and
in these words: 릴리스 PR, 점검, 노트 작성, 버전 커밋, 푸시, CI와 APK, 노트 검토, CI 실패 수정, 배포, 맥 업데이트. Mark a task in progress when you start
its step and completed when it is done (one that does not apply, such as 7 when CI passed, is completed when you pass it). Mighty Code's
role view draws each task as a card on the request's spine, with the agents and background commands started during it beside it.

## Step 0: the release PR (before the release commit)

The unreleased commits go through a PR first, so CI and review see the same tree before `release.py` writes the release commit. Skip
this step only when there is nothing on `main` past `origin/main` and no unreleased commit since the last `(release) x.y.z:` commit.

1. `git switch -c release/<version>`, `git push -u origin release/<version>`, then `gh pr create --base main` by hand. Do not use gstack's
   `/ship`: it bumps VERSION and writes CHANGELOG, which `release.py` and the `release-notes` agent own. Body: why, what changed, scope,
   blast radius, the local checks already run. Never touch VERSION, CHANGELOG or TODOS.md here.
2. Review and drive the PR with the best toolkit installed, checked in this order (MC-116, the user's choice of 2026-11-27):
   - **pstack** (`~/.claude/plugins/cache/*/pstack` exists): follow pstack's `interrogate` skill on the branch diff (adversaries on
     different models; only findings proven by a run survive; it reports, it does not fix), then its `poteto-mode` Babysit playbook in
     `drive` mode on the PR (conflicts → threads → CI; one fresh build for a flake; never rebase, force-push or merge). Read the skill
     and playbook files from the plugin and follow them; Cursor-only steps (Bugbot, Cursor cloud verifiers, `/deslop`) are skipped with a note.
   - **else omc** (`~/.claude/plugins/cache/*/oh-my-claudecode` exists): `oh-my-claudecode:interrogate` on the diff, then
     `oh-my-claudecode:babysit` on the PR.
   - **else**: a read-only review agent (`oh-my-claudecode:code-reviewer` or general-purpose) on `git diff origin/main...release/<version>`,
     `gh pr checks <n> --watch` in the background, and the `ci-failure` agent for each failed run.
3. Fix real findings and the PR's own CI failures on the branch. A failure that also fails on runs without the PR's commits is not the
   PR's: record it as an MC task and say so (until MC-115 is fixed, three jobs of "Native clients — Tauri (0.5)" fail this way).
4. Merge by fast-forwarding: `git switch main && git merge --ff-only release/<version> && git push origin main`. The PR then reads merged.
   The user's release request is the merge decision; with no release request, stop at merge-ready and ask.

## Steps

1. `python scripts/release.py check --version <version>` makes the two note templates.
2. Start the `release-notes` agent in the background with the version, and go on.
3. `python scripts/release.py version --summary "<one line>"` makes the release commit (not pushed).
4. `python scripts/release.py push --yes`.
5. Start these as background commands, side by side:
   - `python scripts/release.py ci`
   - only when the phone app changed since the last release (`git diff --stat <last "(release) x.y.z:" commit> -- mobile`): the EAS preview APK in `mobile/`, `npx eas-cli build -p android --profile preview --non-interactive` (with `--local --output release/<version>/app.apk` to build it on this machine).
6. When the notes agent reports, read both note files and correct what needs it.
7. CI failed: hand the run to the `ci-failure` agent, fix the cause, commit and push the fix, then `python scripts/release.py ci --commit <fix sha>` in the background again.
8. When the notifications say CI (and the APK) are done, in order: `download` → `manifest` (with `--apk <path>` for a new APK) → `page` (show its diff) → `upload --dry-run` → `upload --yes` → `github --yes` → `verify` → `scripts/help/upload.sh` (`--dry-run` first) when the help pages changed.
9. Offer to update this Mac's app to the new version (its own update, which restarts the app); only on the user's yes.
