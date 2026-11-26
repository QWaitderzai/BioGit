import { invoke } from "@tauri-apps/api/core";

// Типы зеркалят структуры из Rust-ядра (biogit-core / src-tauri/commands.rs)

export interface Snapshot {
  hash: string; // "sha256:..."
  timestamp: string; // RFC 3339
  author: string;
  message: string;
  parent: string[];
  branch: string;
  tree: string;
  parameters: Record<string, unknown> | null;
  files_count: number;
}

export interface FileEntry {
  name: string;
  size: number;
}

export interface FileContent {
  text: string | null; // null, если файл двоичный
  is_binary: boolean;
  size: number;
}

export interface BranchInfo {
  name: string;
  snapshots: number;
  latest_message: string | null;
  latest_hash: string | null;
}

export interface RepoState {
  path: string;
  current_branch: string;
  branches: BranchInfo[];
  files: FileEntry[];
  latest: Snapshot | null;
}

export interface ConflictItem {
  file: string;
  reason: string;
}

export interface MergeReport {
  source: string;
  target: string;
  merged: string[];
  conflicts: ConflictItem[];
  snapshot: Snapshot | null;
}

// --- статус рабочих файлов ---

export interface FileStatus {
  name: string;
  state: "unmodified" | "modified" | "added" | "deleted";
  size: number;
  previous_size: number | null;
}

// --- diff ---

export interface DiffLine {
  kind: "equal" | "plus" | "minus";
  text: string;
}

export interface CsvCellChange {
  key: string;
  column: string;
  old: string;
  new: string;
}

export interface CsvDiff {
  header: string[];
  added: string[][];
  removed: string[][];
  changed: CsvCellChange[];
}

export interface FileDiff {
  path: string;
  state: "added" | "deleted" | "modified";
  kind: "text" | "csv" | "binary";
  old_size: number | null;
  new_size: number | null;
  lines: DiffLine[] | null;
  csv: CsvDiff | null;
}

// --- граф истории ---

export interface GraphNode {
  hash: string;
  timestamp: string;
  author: string;
  message: string;
  branch: string;
  parents: string[];
  files_count: number;
  branches: string[];
  lane: number;
  lanes_active: number[];
}

export interface GraphData {
  nodes: GraphNode[];
  lanes_total: number;
}

// --- двухфазное слияние ---

export type ConflictDetail =
  | { type: "text_hunk"; index: number; ours: string[]; theirs: string[] }
  | { type: "csv_cell"; row_key: string; column: string; ours: string; theirs: string }
  | { type: "csv_row"; row_key: string; ours: string[] | null; theirs: string[] | null }
  | { type: "file"; reason: string; ours_exists: boolean; theirs_exists: boolean };

export interface ProposedFile {
  path: string;
  kind: "text" | "csv" | "binary";
  status: "merged" | "conflict";
  variant: string;
  merged_text: string | null;
  csv_header: string[] | null;
  csv_rows: string[][] | null;
  conflicts: ConflictDetail[];
}

export interface MergeProposal {
  source: string;
  target: string;
  base_hash: string | null;
  ours_hash: string;
  theirs_hash: string;
  csv_key: string | null;
  files: ProposedFile[];
}

/** Есть ли конфликты, требующие решения (зеркало MergeProposal::has_conflicts). */
export function proposalHasConflicts(p: MergeProposal): boolean {
  return p.files.some((f) => f.status === "conflict");
}

/** Пути конфликтных файлов. */
export function proposalConflictFiles(p: MergeProposal): string[] {
  return p.files.filter((f) => f.status === "conflict").map((f) => f.path);
}

/** Файлы без конфликтов, в которые будут внесены изменения. */
export function proposalCleanChanges(p: MergeProposal): string[] {
  return p.files.filter((f) => f.status === "merged").map((f) => f.path);
}

export interface FileDecision {
  path: string;
  action: "merged" | "ours" | "theirs";
  hunks: Record<string, string>; // индекс блока → "ours"|"theirs"
  cells: Record<string, string>; // "ключ\x1fколонка" → "ours"|"theirs"
  rows: Record<string, string>; // ключ → "ours"|"theirs"
}

// --- таблицы (Excel уровень A) ---

export interface SheetCell {
  value: string;
  formula: string | null;
}

export interface SheetGrid {
  name: string;
  rows: SheetCell[][];
}

export interface SheetDoc {
  file: string;
  sheets: SheetGrid[];
  active_sheet: number;
  engine: "excel" | "builtin";
  note: string | null;
  dirty: boolean;
}

export interface EditOutcome {
  value: string;
  formula: string | null;
}

export interface ExcelAvailability {
  available: boolean;
  reason: string | null;
  version: string | null;
}

export function shortHash(hash: string): string {
  return hash.replace(/^sha256:/, "").slice(0, 10);
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} Б`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} КБ`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} МБ`;
}

export function formatTime(timestamp: string): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return timestamp;
  return date.toLocaleString("ru-RU", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Буква колонки: 0 → A, 25 → Z, 26 → AA. */
export function colName(index: number): string {
  let n = index + 1;
  let out = "";
  while (n > 0) {
    const rem = (n - 1) % 26;
    out = String.fromCharCode(65 + rem) + out;
    n = Math.floor((n - 1) / 26);
  }
  return out;
}

export const isSheetFile = (name: string): boolean =>
  /\.(xlsx|xls|xlsm|xlsb|ods)$/i.test(name);

export const isCsvFile = (name: string): boolean => /\.(csv|tsv)$/i.test(name);

export const api = {
  openRepo: (path: string) => invoke<RepoState>("open_repo", { path }),

  initRepo: (path: string) => invoke<RepoState>("init_repo", { path }),

  scanFiles: (path: string) => invoke<FileEntry[]>("scan_files", { path }),

  saveSnapshot: (
    path: string,
    message: string,
    author: string,
    parametersJson: string,
  ) =>
    invoke<Snapshot>("save_snapshot", {
      path,
      message,
      author,
      parametersJson,
    }),

  listHistory: (path: string, branch: string) =>
    invoke<Snapshot[]>("list_history", { path, branch }),

  restoreSnapshot: (path: string, hash: string) =>
    invoke<number>("restore_snapshot", { path, hash }),

  listBranches: (path: string) => invoke<BranchInfo[]>("list_branches", { path }),

  createBranch: (path: string, name: string) =>
    invoke<void>("create_branch", { path, name }),

  switchBranch: (path: string, name: string) =>
    invoke<void>("switch_branch", { path, name }),

  mergeBranch: (path: string, source: string) =>
    invoke<MergeReport>("merge_branch", { path, source }),

  // null — пользователь закрыл диалог, папку не выбрал
  pickFolder: () => invoke<string | null>("pick_folder"),

  readFile: (path: string, name: string) =>
    invoke<FileContent>("read_file", { path, name }),

  writeFile: (path: string, name: string, content: string) =>
    invoke<number>("write_file", { path, name, content }),

  // --- новые команды ---

  fileStatus: (path: string) => invoke<FileStatus[]>("file_status", { path }),

  diffWorking: (path: string, hash?: string) =>
    invoke<FileDiff[]>("diff_working", { path, hash: hash ?? "" }),

  diffBetween: (path: string, a: string, b: string) =>
    invoke<FileDiff[]>("diff_between", { path, a, b }),

  historyGraph: (path: string) => invoke<GraphData>("history_graph_cmd", { path }),

  mergePropose: (path: string, source: string, csvKey?: string) =>
    invoke<MergeProposal>("merge_propose", { path, source, csvKey: csvKey ?? null }),

  mergeApply: (
    path: string,
    source: string,
    csvKey: string | null,
    decisions: FileDecision[],
  ) => invoke<MergeReport>("merge_apply", { path, source, csvKey, decisions }),

  excelStatus: () => invoke<ExcelAvailability>("excel_status"),

  sheetOpen: (path: string, name: string) =>
    invoke<SheetDoc>("sheet_open", { path, name }),

  sheetSetActive: (sheet: number) =>
    invoke<void>("sheet_set_active", { sheet }),

  sheetEdit: (sheet: number, row: number, col: number, input: string) =>
    invoke<EditOutcome>("sheet_edit", { sheet, row, col, input }),

  sheetSave: (path: string) => invoke<string>("sheet_save", { path }),

  sheetClose: () => invoke<void>("sheet_close_cmd"),
};
