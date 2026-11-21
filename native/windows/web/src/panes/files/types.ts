// The files pane's model as the host builds it (MainWindow.WebShell.Panes.cs) from Core's FilePaneTree, WorkspaceFiles and
// FilePreviewLoader: the workspace's rules (noise folders, links, caps, filter results) and every note are the host's words.

export interface FileRow {
  path: string;
  name: string;
  depth: number;
  dir: boolean;
  expanded: boolean;
  noise: boolean;
  symlink: boolean;
  /** source, markdown, image or unsupported: the row's symbol. */
  symbol?: string;
  /** An opened folder's caption under it; `reports` when it says the folder could not be read or was cut. */
  caption?: string;
  reports?: boolean;
}

export interface FilesModel {
  available: boolean;
  filter: string;
  filtering: boolean;
  rows: FileRow[];
  /** The notes after the rows (empty, cut, no matches, more results, the root's error). */
  notes: string[];
  selected?: string;
  /** The file the pane last previewed, shown again when it opens. */
  preview?: string;
}

export interface FileImage {
  url: string;
  /** Its size at 1:1: a bitmap's pixels (drawn one to one on the screen, so divided by devicePixelRatio), a vector's points. */
  width: number;
  height: number;
  bitmap: boolean;
  label: string;
  zoomable: boolean;
}

export interface FilePreview {
  path: string;
  name: string;
  /** The file could not be read at all: the reason alone, no head. */
  failure?: string;
  kind: 'source' | 'markdown' | 'image' | 'unsupported';
  encoding?: string;
  text?: string;
  /** Coloured spans as [kind, start, length] triples: 0 keyword, 1 string, 2 comment, 3 number. */
  tokens?: number[];
  lines?: number;
  wraps?: boolean;
  /** The notes over the source (cut, highlight cap, wrapped), and over the rendered Markdown. */
  banners: string[];
  renderedBanners?: string[];
  markdown?: boolean;
  /** True when the file is non-truncated text within the lightweight editor's size cap. */
  editable?: boolean;
  /** SHA-256 hex of raw disk bytes when previewed, used to detect external agent modifications. */
  diskHash?: string;
  /** Line ending convention preserved on save. */
  eol?: 'lf' | 'crlf';
  image?: FileImage;
  size?: string;
  modified?: string;
  reason?: string;
}

export interface FilesSaveResult {
  ok: boolean;
  diskHash?: string;
  size?: string;
  modified?: string;
  encoding?: string;
  eol?: 'lf' | 'crlf';
  reason?: 'conflict' | 'missing' | 'tooLarge' | 'encoding' | 'readOnly' | 'io';
  message?: string;
}

export interface FilesDiskCheckResult {
  status: 'unchanged' | 'modified' | 'missing';
  diskHash?: string;
}

/** One line of a hunk (Core FileDiff.cs): `" "` context, `"+"` added, `"-"` removed; `noNewline` marks a last line without a line break. */
export interface FileDiffLine {
  op: ' ' | '+' | '-';
  text: string;
  noNewline?: boolean;
}

/** A unified-diff hunk; a side with no lines starts at the line before it (0 at the top of the file). */
export interface FileDiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: FileDiffLine[];
}

/**
 * What `referenceChanges` answers (MainWindow.WebShell.cs, args `{ paneId, path, runId? }`): what the request changed in the
 * file, against the original its run kept (`base: 'run'`) or the repository's HEAD (`base: 'head'`); null when there is no
 * diff to show. `added`/`removed` count the whole change even when `truncated` cut the hunks (5,000 lines).
 */
export interface FileChanges {
  base: 'run' | 'head';
  hunks: FileDiffHunk[];
  added: number;
  removed: number;
  truncated: boolean;
  unchanged: boolean;
  /** The run whose original was the base (`run`). */
  runId?: string;
  /** HEAD's short commit (`head`); absent in a repository with no commit yet. */
  headSha?: string;
  /** The run created the file, or HEAD does not have it: every line is added. */
  created?: boolean;
}
