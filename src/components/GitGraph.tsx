import { formatTime, shortHash, type GraphData } from "../api";

// Граф истории в духе git log --graph: дорожки слева (SVG), карточки справа.
// Ядро уже разложило точки по дорожкам (lane/lanes_active) — здесь только отрисовка.

const ROW_H = 52;
const LANE_W = 16;
const DOT_R = 5;

const PALETTE = ["#58a6ff", "#d2a8ff", "#f778ba", "#ffa657", "#7ee787", "#79c0ff", "#ffea7f"];

function laneColor(lane: number): string {
  return PALETTE[lane % PALETTE.length];
}

interface Props {
  graph: GraphData;
  selectedHash: string | null;
  onSelect: (hash: string) => void;
}

export default function GitGraph({ graph, selectedHash, onSelect }: Props) {
  const { nodes, lanes_total } = graph;
  if (nodes.length === 0) {
    return <p className="empty">На этой ветке нет снимков</p>;
  }

  const rowByHash = new Map<string, number>();
  nodes.forEach((n, i) => rowByHash.set(n.hash, i));

  const width = lanes_total * LANE_W + 12;
  const height = nodes.length * ROW_H;
  const x = (lane: number) => lane * LANE_W + 9;
  const y = (row: number) => row * ROW_H + ROW_H / 2;

  const edges: string[] = [];
  for (let i = 0; i < nodes.length; i++) {
    const node = nodes[i];
    for (const parent of node.parents) {
      const j = rowByHash.get(parent);
      if (j === undefined || j <= i) continue;
      const x1 = x(node.lane);
      const x2 = x(nodes[j].lane);
      const y1 = y(i);
      const y2 = y(j);
      if (x1 === x2) {
        edges.push(`M ${x1} ${y1} L ${x1} ${y2}`);
      } else {
        const mid = (y1 + y2) / 2;
        edges.push(
          `M ${x1} ${y1} C ${x1} ${mid}, ${x2} ${mid}, ${x2} ${y2}`,
        );
      }
    }
  }

  return (
    <div className="gitgraph">
      <div className="gitgraph-canvas" style={{ width, height }}>
        <svg width={width} height={height}>
          {/* вертикальные линии активных дорожек */}
          {nodes.map((node, i) =>
            node.lanes_active.map((lane) => (
              <line
                key={`v-${i}-${lane}`}
                x1={x(lane)}
                x2={x(lane)}
                y1={i * ROW_H}
                y2={(i + 1) * ROW_H}
                stroke={laneColor(lane)}
                strokeWidth={2}
                opacity={0.45}
              />
            )),
          )}
          {/* рёбра к родителям */}
          {edges.map((d, i) => (
            <path key={`e-${i}`} d={d} fill="none" stroke="#4a515a" strokeWidth={1.6} />
          ))}
          {/* точки снимков */}
          {nodes.map((node, i) => (
            <circle
              key={`d-${i}`}
              cx={x(node.lane)}
              cy={y(i)}
              r={DOT_R}
              fill={node.parents.length > 1 ? laneColor(node.lane) : "#1c1f24"}
              stroke={laneColor(node.lane)}
              strokeWidth={2}
            />
          ))}
        </svg>
      </div>
      <div className="gitgraph-cards">
        {nodes.map((node) => (
          <button
            key={node.hash}
            type="button"
            className={`timeline-card gitgraph-card ${selectedHash === node.hash ? "timeline-card-active" : ""}`}
            onClick={() => onSelect(node.hash)}
            style={{ height: ROW_H - 6 }}
          >
            <div className="gitgraph-card-main">
              <div className="timeline-hash">{shortHash(node.hash)}</div>
              <div className="timeline-time">{formatTime(node.timestamp)}</div>
            </div>
            <div className="gitgraph-card-msg">
              <div className="timeline-message">{node.message || "(без сообщения)"}</div>
              <div className="timeline-meta">
                {node.author} · {node.files_count} файл(ов)
                {node.parents.length > 1 && " · слияние"}
                {node.branches.length > 0 && ` · ⎇ ${node.branches.join(", ")}`}
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
