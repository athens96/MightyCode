// MC-119: the loop data contract (docs/loops-contract.md), JSON camelCase as the Rust host sends and receives it.

export type LoopState = 'draft' | 'running' | 'paused' | 'passed' | 'failed' | 'stopped';
export type LoopPhase = 'build' | 'verify' | 'repair' | 'done';
export type CriterionLevel = 'L0' | 'L1' | 'L2' | 'L3' | 'L4';
export type CriterionStatus = 'pending' | 'passed' | 'failed';
export type IterationOutcome = 'running' | 'passed' | 'failed' | 'stopped';
export type LoopStopReason =
  | 'passed'
  | 'maxIterations'
  | 'maxMinutes'
  | 'maxCost'
  | 'budget'
  | 'sameFailure'
  | 'person'
  | 'repairEnded'
  | 'paneClosed'
  | (string & {});

export interface CriterionCheck {
  kind: 'command' | 'verifyConfig' | 'checklist' | (string & {});
  name?: string;
  command?: string | null;
  cwd?: string | null;
}

export interface CriterionEvidence {
  level: string;
  summary: string;
  at?: string | null;
  durationMs?: number | null;
}

export interface Criterion {
  id: string;
  text: string;
  check: CriterionCheck;
  level: CriterionLevel | (string & {});
  status: CriterionStatus;
  evidence?: CriterionEvidence | null;
}

export interface LoopLimits {
  maxIterations: number;
  maxMinutes: number;
  maxCostUsd: number;
}

export interface LoopIteration {
  n: number;
  kind: 'build' | 'repair' | (string & {});
  runId?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
  outcome: IterationOutcome;
  failed: string[];
  changedFiles: number;
  added: number;
  removed: number;
  costUsd: number;
}

export interface LoopGuard {
  protectedFiles: number;
  touched: string[];
  testsAtStart: number;
  testsNow: number;
  skipsAdded: number;
  flags: string[];
  /** The guard could not run (an SSH workspace): nothing is locked, the checks still decide. */
  unavailable?: boolean;
}

export interface LoopRecord {
  id: string;
  paneId: string;
  workspaceId: string;
  title: string;
  goal: string;
  state: LoopState;
  phase: LoopPhase | (string & {});
  criteria: Criterion[];
  criteriaHash?: string | null;
  limits: LoopLimits;
  iterations: LoopIteration[];
  guard: LoopGuard;
  costUsd: number;
  startedAt: string;
  finishedAt?: string | null;
  stopReason?: LoopStopReason | null;
  baseCommit?: string | null;
  /** The person asked to pause; the loop stops after its current turn and checks. */
  pausing?: boolean;
  /** Minutes the loop has been driven, paused time left out (what maxMinutes is measured against). */
  activeMinutes?: number;
}

export interface LoopListAnswer {
  enabled: boolean;
  loops: LoopRecord[];
}

/** host → web: pushed whenever a loop record or the enabled switch changes. */
export interface LoopsMessage {
  kind: 'loops';
  enabled?: boolean;
  loops: LoopRecord[];
}

export interface LoopsState {
  enabled: boolean;
  loops: readonly LoopRecord[];
}

export interface LoopStartArgs {
  paneId: string;
  goal: string;
  criteria: Criterion[];
  limits: LoopLimits;
}
