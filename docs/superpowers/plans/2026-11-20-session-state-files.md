# Session State Files Implementation Plan

**Goal:**
- Split the one state file into a light index and one body file per pane (session).
- Write only the panes that changed.
- Read a pane's body only when something needs it.
- Lay the ground for the remote mirror and the phone to fetch, cache and later delta-sync one pane at a time.

**Why (measured on 2026-11-20):**
- The real state file is 6.4 MB with 11 panes. `graphRuns` is 4.3 MB and `logs` is 2.0 MB. Everything else is about 70 KB, and the index part is about 10 KB.
- While any agent runs, the whole 6.4 MB is normalized, encoded and fsynced every 1.5 s (`store::schedule`).
- 0.5.6 made a too-large state save again by cutting the written copy. That is a stopgap; this plan replaces it.

**Decisions (agreed with the user):**
- The unit is the pane, not the workspace. One workspace with three big panes would be 5.8 MB again, and the mirror and the phone already ask for one pane's detail at a time.
- The index stays `version: 1`. A downgraded app (0.5.6) still opens it; its panes just show no history.
- A closed pane's body file is deleted after the next index save that no longer lists it (as today, closing a pane drops its data).
- Phases 1, 2 and 3 are all in scope. The Mac is checked first; the code is shared Rust, so Windows comes with it.

## Layout

```
<profile>/
  workspace-state.json            index: workspaces, layout, settings; per pane its small fields + bodyRevision + digest
  workspace-state.json.bak
  sessions/                       0700
    <pane id>.json                0600: {version:1, sessionId, bodyRevision, savedAt, <body fields>}
    <pane id>.json.bak
```

- **Body fields:** the ones that grow, with the same wire names as today, so `RunSession`'s reader and writer are reused:
  - `logs`
  - `graphRuns`
  - `shellRuns`
  - `planHistory`
  - `todoProgress`
  - `backgroundWork`
  - `draft`
- **Index pane entry:** every other `RunSession` field, plus:
  - `bodyRevision`: grows by one on every body write.
  - `digest`: see Phase 2.
- **Index root:** `"sessionFiles": 1` marks a split state.
- **Limits:**
  - The index is at most 8 MiB.
  - Each body file is at most 8 MiB. `encode_fitting`'s cutting moves from the whole state to one pane.
- **Budgets:**
  - The per-pane graph budget (`LIVE_HISTORY_LIMIT`, 2 MiB) and the 400-entry log cap are unchanged.
  - The log-text `Budget { text: 2 MiB }` that this PC's panes share today (normalize.rs, `normalize`) becomes per pane, so a pane's saved body never depends on another pane.
- **Files panes, work-map panes and remote mirror panes:** never written, as today.

## Phase 1: split on disk (all bodies still loaded at start)

**Save (`Store::save`):**
1. Normalize per pane, reusing the last normalized result of a pane whose `Arc<RunSession>` is the same object as last time (a cache keyed by id: input Arc, output Arc).
2. A pane is dirty when its normalized Arc is not the one last written. For each dirty pane, encode its body (cut to fit), bump `bodyRevision` and `atomic_write` `sessions/<id>.json` with `.bak`.
3. Write the index (only when its bytes changed).
4. Delete body files (and `.bak`) of panes the index no longer lists.

A crash between steps 2 and 3 leaves a body newer than the index, which is harmless.

**Load (`Store::load`):**
1. Read the index as today (`read_state`: checks, kept workspaces, gemini migration).
2. For each pane, read its body file and lay it on the index entry before normalize.
3. If an index entry carries body fields inline, an older app or the pre-split file wrote it:
   - With no body file, use the inline fields.
   - With a body file and no `sessionFiles` marker (a downgrade, then an upgrade), merge the two by id: graph runs and log entries only the file has go first, then the inline ones. Nothing the older app wrote in between is lost.

**Damaged body file:**
- A file holding NUL, or one that does not decode, is moved aside to `<id>.json.unreadable-<time>-<id8>`.
- Its `.bak` is restored if it reads; otherwise the pane opens with an empty body.
- A notice in the user's language says which pane (new locale key).
- Other panes are untouched. This replaces "the whole state stops loading" for body files; the index keeps today's rules.

**Migration:**
- A load of an unsplit state (no marker) that saves for the first time copies the old file once to `workspace-state.json.before-session-files-<yyyyMMdd-HHmmss>`, then writes the split form.

**Permissions:**
- Unix: the profile's `sessions/` is 0700. Body files, the index and their temporary files are created 0600 (`OpenOptionsExt::mode`).
- Windows: the per-user profile's ACL as today.

**Contract and tests:**
- `native/contracts/README.md` and `README.md`'s state section describe the split.
- Fixtures that read `workspace-state.json` as one file go through the store or a helper that joins the bodies:
  - mighty-core `tests/state.rs`, `load.rs`, `state_mac.rs`, `parity.rs`
  - mighty-remote `remote_mirror.rs`, `phone_parity.rs`
  - mighty-workmap `work_dashboard.rs`
- New tests:
  - Only dirty panes are written.
  - The index and bodies round-trip.
  - Migration with its backup.
  - Downgrade then upgrade merges.
  - A crash between body and index.
  - A damaged body (NUL, undecodable, with and without `.bak`).
  - A closed pane's file deleted.
  - Per-pane size cutting.
  - File modes.

## Phase 2: pane digest (what list-level readers need without the body)

`SessionDigest`, saved in the index entry and computed again whenever the pane's Arc changes (a cache keyed by id and Arc pointer), holds:
- `logCount`
- `tail`: the last few entries, each trimmed (what the phone summary, dashboard card and revisions read)
- `lastTimestamp`
- `lastRequest`: the newest user request text, trimmed
- `runIds`: every graph run id
- `graphDigest`: `mobile::digest`

Readers move to the digest:
- phone `summary` / `revisions` / push preview
- mirror `summaries_now` / `support::revision`
- dashboard `make_card`
- `FileOriginalStore::kept_runs_of` (this matters: `retain_originals` runs every 2 s and deletes the originals of runs it does not see)
- the Mighty Pet status list

The mirror revision becomes the digest's fingerprint plus `bodyRevision`.

## Phase 3: bodies loaded on demand

**State of a body:**
- A pane whose body is not in memory carries `body_on_disk = true` (never serialized) and no body fields.
- `BodyLoader` (store directory → `Option<SessionBody>`) is handed to the run service and the host.

**Startup:**
- The index is read.
- Bodies are read for the panes of the active workspace's layout, and for any pane waiting for an answer.
- The rest stay on disk.

**Mutation points load first:**
- `RunService::receive_locked`, `try_update`, `update_pane`, `update_draft` and `View::update` ensure the target pane's body before applying a change.
- A run's start ensures its pane.
- A save never writes the body of a pane still on disk. As a guard, a dirty pane still flagged `body_on_disk` is merged with its file by id instead of replacing it.

**Readers that need a body load it:**
- a pane becoming visible (layout, tab or workspace switch) before its projection
- phone `detail` and `entries`
- mirror `detail`
- the work map when it opens (all panes of its workspace)
- review `originals_run` for the pane asked about
- `deep_link` for the pane it targets
- the Mighty Pet's focused pane
- history `older` / `load_older` (visible panes only)

**Page:**
- `ready()` pushes the panes that are loaded.
- A pane loaded later reaches the page through the normal delta flush (logs and graphRuns are diffed by id).

Out of scope for now: unloading idle panes again.

## Remote, prepared here and built later

- The body file schema is the transfer unit: the mirror detail can become "pane X's body at revision N".
- `bodyRevision` survives restarts, so `since=` keeps working across them. A later delta ("runs after revision N") needs no format change.
- Reserved path for caching a connected host's panes: `remote-cache/<connection id>/<host pane id>.json`, keyed by revision. Not written yet.

## Checks

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- Windows: `CARGO_TARGET_DIR=/tmp/mc-t-win cargo clippy --target x86_64-pc-windows-msvc -p mighty-core -p mighty-run -p mighty-tauri`
- Web: `npx tsc --noEmit -p .` and `npx vitest run`
- `node scripts/check-locales.js` (new notice keys, copied to mobile and macOS)
- Mac build staged with `scripts/install-macos.sh` through launchctl. Never a second app instance.
