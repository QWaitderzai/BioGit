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
};
