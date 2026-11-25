import { useMemo, useState, type ReactNode } from "react";
import { formatSize, type FileEntry } from "../api";

// проводник в духе vscode: бэкенд отдаёт файлы плоским списком,
// из него тут собирается дерево, клик по файлу открывает редактор

interface TreeNode {
  name: string;
  path: string; // путь от корня рабочей папки
  isDir: boolean;
  size: number;
  children: TreeNode[];
}

// на входе пути вида "results/out.csv", раскладываем их по папкам
function buildTree(files: FileEntry[]): TreeNode[] {
  const roots: TreeNode[] = [];
  const dirIndex = new Map<string, TreeNode>();

  const ensureDir = (segments: string[]): TreeNode => {
    let list = roots;
    let key = "";
    let node: TreeNode | undefined;
    for (const segment of segments) {
      key = key ? `${key}/${segment}` : segment;
      node = dirIndex.get(key);
      if (!node) {
        node = { name: segment, path: key, isDir: true, size: 0, children: [] };
        dirIndex.set(key, node);
        list.push(node);
      }
      list = node.children;
    }
    return node as TreeNode;
  };

  for (const file of files) {
    const parts = file.name.split("/");
    const parent = parts.length > 1 ? ensureDir(parts.slice(0, -1)) : null;
    (parent ? parent.children : roots).push({
      name: parts[parts.length - 1],
      path: file.name,
      isDir: false,
      size: file.size,
      children: [],
    });
  }

  const sortRec = (nodes: TreeNode[]) => {
    // папки сверху, внутри по алфавиту
    nodes.sort((a, b) =>
      a.isDir === b.isDir ? a.name.localeCompare(b.name) : a.isDir ? -1 : 1,
    );
    for (const node of nodes) if (node.isDir) sortRec(node.children);
  };
  sortRec(roots);
  return roots;
}

// цвет иконки подбираем по расширению (био-форматы, таблицы, текст)
function extClass(name: string): string {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (["fasta", "fa", "fna", "ffn", "faa", "frn"].includes(ext)) return "ext-bio";
  if (["csv", "tsv"].includes(ext)) return "ext-data";
  if (ext === "json") return "ext-json";
  if (["md", "txt", "log"].includes(ext)) return "ext-doc";
  return "ext-other";
}

function Caret({ collapsed }: { collapsed: boolean }) {
  return (
    <svg
      className={`tree-caret ${collapsed ? "tree-caret-collapsed" : ""}`}
      width="10"
      height="10"
      viewBox="0 0 16 16"
      aria-hidden="true"
    >
      <path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg className="tree-icon" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M1.5 4A1.5 1.5 0 0 1 3 2.5h3.2L7.7 4H13a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 13H3A1.5 1.5 0 0 1 1.5 11.5Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}

function FileIcon({ className }: { className: string }) {
  return (
    <svg
      className={`tree-icon ${className}`}
      width="14"
      height="14"
      viewBox="0 0 16 16"
      aria-hidden="true"
    >
      <path
        d="M4 1.5h5.2L12.5 4.8V14a.5.5 0 0 1-.5.5H4A.5.5 0 0 1 3.5 14V2a.5.5 0 0 1 .5-.5Z M9 1.5V5h3.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}

interface Props {
  files: FileEntry[];
  activeFile: string | null;
  onOpenFile: (name: string) => void;
}

export default function FileExplorer({ files, activeFile, onOpenFile }: Props) {
  const tree = useMemo(() => buildTree(files), [files]);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const toggleDir = (path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  const renderNodes = (nodes: TreeNode[], depth: number): ReactNode[] =>
    nodes.map((node) => {
      const indent = { paddingLeft: 8 + depth * 14 };
      if (node.isDir) {
        const isCollapsed = collapsed.has(node.path);
        return (
          <div key={`dir:${node.path}`}>
            <button
              type="button"
              className="tree-row tree-row-dir"
              style={indent}
              onClick={() => toggleDir(node.path)}
              title={node.path}
            >
              <Caret collapsed={isCollapsed} />
              <FolderIcon />
              <span className="tree-name">{node.name}</span>
            </button>
            {!isCollapsed && renderNodes(node.children, depth + 1)}
          </div>
        );
      }
      return (
        <button
          type="button"
          key={node.path}
          className={`tree-row ${activeFile === node.path ? "tree-row-active" : ""}`}
          style={indent}
          onClick={() => onOpenFile(node.path)}
          title={`${node.path} · ${formatSize(node.size)}`}
        >
          <span className="tree-caret-spacer" aria-hidden="true" />
          <FileIcon className={extClass(node.name)} />
          <span className="tree-name">{node.name}</span>
          <span className="tree-size">{formatSize(node.size)}</span>
        </button>
      );
    });

  if (files.length === 0) {
    return (
      <div className="tree">
        <p className="empty tree-empty">
          Папка пуста.
          <br />
          Скопируйте сюда файлы данных и нажмите «Обновить».
        </p>
      </div>
    );
  }

  return <div className="tree">{renderNodes(tree, 0)}</div>;
}
