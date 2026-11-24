// What the host pushes of a pane's composer (MainWindow.WebShell.Composer.cs WebComposerState): read from the never-shown
// WinUI composer the page's draft is mirrored into, so every rule (block reasons, busy, queue, attachments) stays C#'s.
import type { Pills } from './menus';
import type { QueueRemote, RunNow } from './queue';

export type { RunNow, QueueRemote } from './queue';

/** A row delegation holds for the pane; only the first carries 바로 실행 (the host sends null for the rest). */
export interface QueueHeld {
  id: string;
  text: string;
  runNow: RunNow | null;
}

export interface QueueItem {
  id: string;
  text: string;
  attachments: string[];
  /** What 바로 실행 does on the row (Core QueueRunNow); null draws no button. */
  runNow: RunNow | null;
  /** MC-49: the words are not edited (an allowed card's follow-up; a host's row whose words came cut); absent: they may be. */
  locked?: boolean;
}

export interface ComposerAttachment {
  id: string;
  name: string;
  mediaType: string;
  size: number;
}

export interface ComposerState {
  paneId: string;
  kind: string;
  busy: boolean;
  starting: boolean;
  stopping: boolean;
  canSend: boolean;
  checkingModels: boolean;
  attachmentsLoading: boolean;
  placeholderKey: string;
  /** Why the draft cannot run, already in the language (the C# reason); null when nothing blocks. */
  blocked: string | null;
  /** The blocked row offers the CLI's install. */
  install: boolean;
  /** The provider's label while a background CLI update holds sends. */
  hold: string | null;
  /** `worktree`: the pane runs, and Enter runs the words now in a new worktree of 실행 중 보내기's agent (`busySend`). */
  primary: 'send' | 'stop' | 'queue' | 'worktree';
  primaryEnabled: boolean;
  /** The worktree button: the second send's child of the pane's own agent. */
  worktree: boolean;
  worktreeEnabled: boolean;
  queue: {
    held: QueueHeld[];
    items: QueueItem[];
    waitsOnBackground: string | null;
    /** MC-49: the rows are another computer's (a mirrored pane's host queue); absent for this PC's own. */
    remote?: QueueRemote | null;
  };
  attachments: ComposerAttachment[];
  attachmentError: string | null;
  attachmentsAllowed: boolean;
  /** The toolbar's pills; null on a shell pane. */
  pills: Pills | null;
  /** The pane header's word (in its tone) and the figures after the clock, with their tooltip. */
  header: { word: string; tone: string; timed: boolean; rest: string; help: string };
  /** The pane goes on from an earlier session: the toolbar shows the resume mark. */
  resumed?: boolean;
  /** A Claude pane's status-line toggle, on or off (null for a remote pane, which has no status line); null elsewhere. */
  toggles?: { statusLine: boolean | null } | null;
  /** A Mighty Code pane's style row (M/SessionPaneView.swift composer): the chosen style's name, its source badge, what it is for; null elsewhere. */
  style?: { label: string; badge: string | null; hint: string; enabled: boolean } | null;
  /** The chosen style's guide panel under that row (MainWindow.WebShell.Guided.cs); null or absent without a style. */
  guided?: GuidedPanel | null;
  /** The worktree menu's agents while the pane offers a worktree: 실행 중 보내기's choices (drawn only while it runs) and the queued rows' agent pick; none hides both. */
  worktreeAgents: WorktreeAgent[];
  /** 실행 중 보내기: the agent whose new worktree Enter runs the words in now; absent: Enter queues them (대기열). */
  busySend?: string | null;
}

/** A phase in the style's bar: the one the pane is in, one already reached, or one still ahead. */
export interface GuidedPhase {
  id: string;
  title: string;
  state: 'current' | 'reached' | 'ahead';
}

/** A state widget the style's sources read (§1.16): a bar with its count, a list, a line, or the pane's background tasks. */
export type GuidedWidget =
  | { kind: 'progressBar'; fraction: number; text: string }
  | { kind: 'list'; items: string[] }
  | { kind: 'text'; text: string }
  | { kind: 'taskList'; tasks: { text: string; detail: string; running: boolean }[] };

/** An action chip (M/GuidedActionChip.swift:36-56): its words in the language, and whether it can be pressed now. */
export interface GuidedAction {
  id: string;
  title: string;
  /** The manifest's glyph or the icon's symbol; null for none. */
  mark: string | null;
  userInvoked: boolean;
  readOnly: boolean;
  emphasised: boolean;
  /** The ink variable an emphasised chip is tinted with (agent-text, task-text, …, accent). */
  tint: string;
  enabled: boolean;
  help: string;
}

/**
 * The style's guide panel (M/GuidedPanel.swift:94-127): either the strip of a style that still needs a yes, or the phase
 * bar and — unless a question or a plan's answers wait (then `waiting`, and the cards draw them) — what is missing, the
 * state widgets, the group map, the chips or a spinner, the reset chip and the guidance line.
 */
export interface GuidedPanel {
  approval?: { name: string; source: string | null; canView: boolean };
  phases?: GuidedPhase[];
  waiting?: boolean;
  /** The command Enter sends in place of the draft, for the prefix chip; null when Enter sends the draft as it is. */
  enter?: string | null;
  setup?: { missing: string[]; hint: string | null; installCommand: string | null } | null;
  widgets?: GuidedWidget[] | null;
  groups?: { id: string; title: string; axis: string; help: string; chosen: boolean }[] | null;
  groupQuestion?: string | null;
  spinner?: boolean;
  actions?: GuidedAction[];
  /** Under a group map the chips are a catalogue grid instead of one sideways row. */
  catalogue?: boolean;
  reset?: { kind: 'new' | 'cancel'; title: string; help: string | null } | null;
  guidance?: string | null;
}

/** One agent of the worktree menu: its provider id, its name, the row's words (the mode its child starts in, or why it cannot run now) and whether it can run now. */
export interface WorktreeAgent {
  agent: string;
  label: string;
  detail: string;
  enabled: boolean;
}

/**
 * What `send` answers (SubmitReceipt.SendAnswer): whether the host took it, what it did with the draft, the user entry and graph
 * run of a request that started at once (two ids on Windows; none for a start that went out with held items, and no run for a
 * pane that draws no diagram here), the page's own clientId back, the editor's text after it, and the composer then. An older
 * host sends only outcome, draft and state.
 */
export interface SendResult {
  accepted?: boolean;
  outcome: 'started' | 'notStarted' | 'queued' | 'worktree' | 'answered' | 'builtin' | 'blocked' | 'none';
  entryId?: string;
  runId?: string;
  clientId?: string;
  draft: string;
  state: ComposerState;
}

export interface AttachmentPreview {
  name: string;
  mediaType: string;
  size: number;
  image?: string;
  text?: string;
}
