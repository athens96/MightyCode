# Loop contract (MC-118 host, MC-119 page)

The loop skeleton of 0.7.0. The host (Rust, `native/tauri`) owns the loop; the page (`native/windows/web/src`) shows it and asks for
actions. This file is the contract both sides build against. Design: MightyWork wiki "루프 설계: 검증의 확실성과 다각도 개선".

## What a loop is

A loop drives one agent pane toward a goal: it sends the goal, and after each turn the app runs the loop's checks. While a check fails
it sends a repair request to the same pane and checks again, until every criterion passes or a stop condition is met. Each turn is an
ordinary request of the pane, so the flow, timeline, result card and audit draw it as before. Nothing changes for a pane without a loop.

- One running loop per pane. A pane with a running loop gets no turn-end verification of its own (the loop's checks replace it).
- Loops are opt-in: the switch (default off, beta; `enabled` in `loopList`'s answer and the push) hides every entry when off. The
  switch itself is in Settings › 에이전트, which the host draws; the page only reads `enabled`.

## The record (`LoopRecord`, JSON, camelCase)

```jsonc
{
  "id": "loop-…",                // stable
  "paneId": "…",
  "workspaceId": "…",
  "title": "CI 초록불까지",        // short, from the goal's first line (≤ 80 chars)
  "goal": "…",                   // the person's words, sent to the agent
  "state": "running",            // see States
  "phase": "verify",             // "build" | "verify" | "repair" | "done"
  "criteria": [ /* Criterion */ ],
  "criteriaHash": "ab12…",       // sha256 (16 hex) of the approved criteria; they are locked while the loop runs
  "limits": { "maxIterations": 10, "maxMinutes": 120, "maxCostUsd": 5 }, // 1–50, 1–1440, 0.1–500; outside these the start is refused
  "iterations": [ /* Iteration */ ],
  "guard": { /* Guard */ },
  "costUsd": 1.20,               // the loop's turns, from the usage ledger
  "activeMinutes": 14.5,         // minutes the loop has actually run (paused time not counted); maxMinutes is measured against it
  "pausing": false,              // true from loopPause until the current turn and its checks end (state stays running)
  "startedAt": "2026-11-28T10:00:00Z",
  "finishedAt": null,
  "stopReason": null,            // see Stop reasons; set when state is passed/failed/stopped
  "baseCommit": "a91c2e0…"       // HEAD of the pane's run folder when the loop started (null outside git)
}
```

### Criterion

```jsonc
{
  "id": "c1",
  "text": "단위 테스트가 모두 통과한다",
  "check": { "kind": "command", "name": "test", "command": "npm test", "cwd": null },
  "level": "L2",                 // the level it must reach: "L1" | "L2" | "L3" | "L4"
  "status": "failed",            // "pending" | "passed" | "failed"
  "evidence": { "level": "L2", "summary": "12 passed, 1 failed", "at": "…", "durationMs": 4100 } // or null
}
```

`check.kind`:
- `command`: a shell command run in the pane's run folder (the same runner as `.mighty.yaml` `verify.commands`); passes on exit 0.
- `verifyConfig`: the approved `.mighty.yaml` `verify` commands and browser check, all of them; `name` is the command's key.
- `checklist`: the agent's own checklist item (`text`), passes when the pane's newest plan marks it done. Level L0/L1 only: it never
  makes a loop pass on its own; at least one `command` or `verifyConfig` criterion is required.

Levels (the wiki's evidence ladder; 0.7.0 runs L1–L4, L5 comes in 0.7.1): L1 build/type/lint, L2 existing tests, L3 a test written for
this goal, L4 running it for real (a command's output, the browser check).

### Iteration

```jsonc
{
  "n": 3,
  "kind": "repair",              // "build" (the goal) | "repair"
  "runId": "…",                  // the pane's request; the page opens it with the existing reveal
  "startedAt": "…", "finishedAt": "…",
  "outcome": "failed",           // "running" | "passed" | "failed" | "stopped"
  "failed": ["test"],            // criterion ids that failed after this turn
  "changedFiles": 2, "added": 18, "removed": 6,
  "costUsd": 0.45
}
```

### Guard (anti-gaming, first cut)

```jsonc
{
  "protectedFiles": 42,          // test files and verify scripts locked at start
  "touched": ["src/foo.test.ts"],// protected files the agent changed (a check fails while this is not empty)
  "testsAtStart": 310, "testsNow": 309,
  "skipsAdded": 1,
  "flags": ["testsRemoved", "skipAdded", "protectedChanged"], // empty when clean
  "unavailable": false           // true in an SSH workspace: the guard is off (no lock, no flags), the criteria are still checked
}
```

A round with any guard flag fails, whatever the commands said, and the repair request names the flag.

Test files and verify scripts are locked. A Rust source with inline tests (`#[cfg(test)]`) is counted for `testsAtStart`/`testsNow`
and `skipsAdded`, but not locked, since the agent must be free to change the code around them.

## States

| state | meaning |
|---|---|
| `draft` | made by `loopDraft`, not started (only lives in the sheet) |
| `running` | a turn or a check is under way (`phase` says which) |
| `paused` | stopped after the current turn by the person or by the app restarting; `loopResume` continues |
| `passed` | every criterion passed with a clean guard |
| `failed` | a stop condition ended it before a pass |
| `stopped` | the person stopped it |

## Stop reasons

`passed`, `maxIterations`, `maxMinutes`, `maxCost`, `budget` (the daily budget), `sameFailure` (the same failure twice), `person`
(the person sent something to the pane or stopped its run), `repairEnded` (a turn failed or waits for the person), `paneClosed`.

## Page methods (the page calls; JSON in, JSON out; errors are strings)

| method | args | answer |
|---|---|---|
| `loopList` | `{}` | `{ "enabled": bool, "loops": LoopRecord[] }` newest first, at most 50 |
| `loopDraft` | `{ "paneId", "goal" }` | a `LoopRecord` in `draft`: its `criteria` proposed from the workspace's approved `verify` (one `verifyConfig` criterion per command, plus the browser check) and its default `limits` |
| `loopStart` | `{ "paneId", "goal", "criteria": Criterion[], "limits" }` | the started `LoopRecord`. Starting is the person's approval of the criteria; they are locked (hash) from here |
| `loopPause` | `{ "id" }` | the record |
| `loopResume` | `{ "id" }` | the record |
| `loopStop` | `{ "id" }` | the record |
| `loopForget` | `{ "id" }` | `{}`; a finished loop leaves the list |
| `loopSetEnabled` | `{ "on": bool }` | `{ "enabled": bool }`; the same switch Settings › 에이전트 › 루프 (베타) sets |

Errors the page shows as they come: no such pane, the pane already has a running or paused loop, the pane is busy, no command
criterion, loops are off, a limit out of range (`loop.host.error.limits`; a mistyped limit is refused, never replaced by the default).

`loopPause` takes effect after the current turn and its checks; `loopStop` stops at once (the agent's turn that is running goes on as
an ordinary request). `loopResume` on a paused loop checks the folder as it is first, then repairs; pressed while the record is still
`pausing`, it cancels the pause. A paused loop does not hold the pane: an ordinary request in it gets the usual verify.

## Push

The host pushes `loops` whenever a record changes: `{ "kind": "loops", "loops": LoopRecord[] }` (the same list `loopList` answers).
The page keeps it in its store and draws the loop tab, the detail and the pane's band from it.

The pane's band comes from this list: the record whose `paneId` is the pane and whose `state` is `running` or `paused` (at most one).
The pane model is not changed.

## CLI

`mighty loop start <pane> --goal "…" [--check "name=command"]… [--max-iterations N] [--max-minutes M] [--max-cost USD]`,
`mighty loop status [<id>]`, `mighty loop stop <id>` (JSON out, like the other `mighty` commands).

## Persistence

`<data>/loops.json` (`{ enabled, loops }`, the newest 50; past that the oldest finished go), each loop's guard baseline in
`<data>/loop-guards/<id>.json`. On start the app reads it; a loop that was `running` comes back `paused` (`stopReason` unset), so the
person resumes it.

## Locale keys

Host notes and errors use `loop.host.*`; the page's words use `loop.ui.*`. Each side adds only its own prefix, in all four
`locales/*.json` (and the mirrors), so the two branches do not edit the same lines.
