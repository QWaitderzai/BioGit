//! Граф истории: все снимки репозитория как DAG с раскладкой линий.
//!
//! Ядро выполняет раскладку по «дорожкам» (как `git log --graph`): каждая
//! дорожка — вертикальная линия; снимок ставится на дорожку, где его ждут,
//! первый родитель наследует дорожку снимка, остальные родители получают
//! новые. Фронтенд просто отрисовывает полученные координаты.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::error::Result;
use crate::object::Snapshot;
use crate::snapshot::Repo;

/// Узел графа с координатами для отрисовки.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub hash: String,
    pub timestamp: String,
    pub author: String,
    pub message: String,
    pub branch: String,
    pub parents: Vec<String>,
    pub files_count: usize,
    /// Имена ветвей, указывающих на этот снимок (кончик ветки).
    pub branches: Vec<String>,
    /// Дорожка точки снимка.
    pub lane: usize,
    /// Дорожки, активные после этого ряда (вертикальные линии).
    pub lanes_active: Vec<usize>,
}

/// Полный граф репозитория.
#[derive(Debug, Clone, Serialize)]
pub struct GraphData {
    /// Узлы от нового к старому.
    pub nodes: Vec<GraphNode>,
    /// Число дорожек (ширина рисунка).
    pub lanes_total: usize,
}

/// Собирает граф всех снимков, достижимых от кончиков всех ветвей.
pub fn history_graph(repo: &Repo) -> Result<GraphData> {
    // 1. Кончики ветвей
    let mut tips: BTreeSet<String> = BTreeSet::new();
    let mut tip_branches: HashMap<String, Vec<String>> = HashMap::new();
    for info in repo.list_branches()? {
        if let Some(hash) = info.latest_hash {
            tips.insert(hash.clone());
            tip_branches.entry(hash).or_default().push(info.name);
        }
    }

    // 2. Обход всех родителей
    let mut nodes: HashMap<String, Snapshot> = HashMap::new();
    let mut stack: Vec<String> = tips.iter().cloned().collect();
    while let Some(hash) = stack.pop() {
        if nodes.contains_key(&hash) {
            continue;
        }
        let snap = repo.snapshot(&hash)?;
        for parent in &snap.parent {
            stack.push(parent.clone());
        }
        nodes.insert(hash, snap);
    }

    if nodes.is_empty() {
        return Ok(GraphData {
            nodes: Vec::new(),
            lanes_total: 0,
        });
    }

    // 3. Топологический порядок: сначала дети, потом родители; среди готовых —
    // новые сверху. Обычная сортировка по времени ломает раскладку при равных
    // секундах (родитель мог бы выйти раньше своего ребёнка).
    let mut pending_children: HashMap<String, usize> = HashMap::new();
    for snap in nodes.values() {
        for parent in &snap.parent {
            *pending_children.entry(parent.clone()).or_insert(0) += 1;
        }
    }
    let mut ready: Vec<&Snapshot> = nodes
        .values()
        .filter(|s| pending_children.get(&s.hash).copied().unwrap_or(0) == 0)
        .collect();
    let mut ordered: Vec<&Snapshot> = Vec::with_capacity(nodes.len());
    while !ready.is_empty() {
        let best = ready
            .iter()
            .enumerate()
            .max_by(|a, b| {
                (a.1.timestamp.as_str(), a.1.hash.as_str())
                    .cmp(&(b.1.timestamp.as_str(), b.1.hash.as_str()))
            })
            .map(|(i, _)| i)
            .expect("готовых узлов нет — цикл в истории");
        let snap = ready.remove(best);
        for parent in &snap.parent {
            if let Some(count) = pending_children.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    if let Some(parent_snap) = nodes.get(parent) {
                        ready.push(parent_snap);
                    }
                }
            }
        }
        ordered.push(snap);
    }

    // 4. Раскладка дорожек
    // lanes[i] — хеш снимка, которого ждём на дорожке i (None — свободна)
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut out: Vec<GraphNode> = Vec::with_capacity(ordered.len());
    let mut lane_of: HashMap<String, usize> = HashMap::new();

    for snap in &ordered {
        // дорожка для этого снимка
        let lane_idx = match lanes.iter().position(|h| h.as_deref() == Some(snap.hash.as_str())) {
            Some(idx) => idx,
            None => {
                lanes.push(Some(snap.hash.clone()));
                lanes.len() - 1
            }
        };

        // родители: первый наследует дорожку, остальные — новые/существующие
        let mut parent_lanes: Vec<(String, usize)> = Vec::new();
        for (i, parent) in snap.parent.iter().enumerate() {
            let target = if i == 0 {
                lane_idx
            } else {
                match lanes.iter().position(|h| h.as_deref() == Some(parent.as_str())) {
                    Some(existing) => existing,
                    None => {
                        lanes.push(Some(parent.clone()));
                        lanes.len() - 1
                    }
                }
            };
            lanes[target] = Some(parent.clone());
            parent_lanes.push((parent.clone(), target));
        }
        if snap.parent.is_empty() {
            lanes[lane_idx] = None;
        }

        // родители, не найденные в графе (битые ссылки) — игнорируем при отрисовке,
        // но заносим их дорожки в lane_of, чтобы фронтенд не упал
        for (hash, lane) in &parent_lanes {
            lane_of.entry(hash.clone()).or_insert(*lane);
        }
        lane_of.insert(snap.hash.clone(), lane_idx);

        let lanes_active: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, h)| h.is_some())
            .map(|(i, _)| i)
            .collect();

        out.push(GraphNode {
            hash: snap.hash.clone(),
            timestamp: snap.timestamp.clone(),
            author: snap.author.clone(),
            message: snap.message.clone(),
            branch: snap.branch.clone(),
            parents: snap.parent.clone(),
            files_count: snap.files_count,
            branches: tip_branches.get(&snap.hash).cloned().unwrap_or_default(),
            lane: lane_idx,
            lanes_active,
        });
    }

    // Финальные дорожки родителей (после всех рядов) для edge-координат
    for (hash, lane) in lane_of.iter_mut() {
        if let Some(final_lane) = lanes.iter().position(|h| h.as_deref() == Some(hash.as_str())) {
            *lane = final_lane;
        }
    }

    let lanes_total = lanes.len();
    Ok(GraphData {
        nodes: out,
        lanes_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Repo;

    fn repo_in_tmp() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().expect("временная папка");
        let repo = Repo::init(dir.path()).expect("инициализация");
        (dir, repo)
    }

    #[test]
    fn граф_линейной_истории_одна_дорожка() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        repo.save_snapshot("раз", "Аня", &serde_json::Value::Null).expect("снимок");
        std::fs::write(repo.root().join("a.txt"), "2").expect("файл");
        repo.save_snapshot("два", "Аня", &serde_json::Value::Null).expect("снимок");

        let graph = history_graph(&repo).expect("граф");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.lanes_total, 1);
        assert!(graph.nodes.iter().all(|n| n.lane == 0));
        // у нового снимка линия продолжается к родителю, у корня дорожка закрывается
        assert_eq!(graph.nodes[0].lanes_active, vec![0]);
        assert!(graph.nodes[1].lanes_active.is_empty());
    }

    #[test]
    fn граф_с_веткой_и_слиянием_содержит_всех() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("f.txt"), "base").expect("файл");
        let s1 = repo.save_snapshot("база", "Аня", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-B").expect("ветка");

        // main меняет f.txt, ветка — отдельный g.txt: слияние без конфликтов
        std::fs::write(repo.root().join("f.txt"), "main-v2").expect("файл");
        let s2 = repo.save_snapshot("main: v2", "Аня", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-B").expect("переключение");
        std::fs::write(repo.root().join("g.txt"), "hypothesis-only").expect("файл");
        let s3 = repo.save_snapshot("B: новый файл", "Борис", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = crate::merge_branch(&repo, "hypothesis-B").expect("слияние");
        let merge = report.snapshot.as_ref().expect("снимок слияния");

        let graph = history_graph(&repo).expect("граф");
        assert_eq!(graph.nodes.len(), 4, "все четыре снимка в графе");
        assert_eq!(graph.lanes_total, 2, "линия ветки и линия main");

        let merge_node = graph.nodes.iter().find(|n| n.hash == merge.hash).unwrap();
        assert_eq!(merge_node.parents.len(), 2, "у слияния два родителя");
        assert!(merge_node.branches.contains(&"main".to_string()), "main указывает на слияние");

        let tip_of_b = graph.nodes.iter().find(|n| n.hash == s3.hash).unwrap();
        assert!(tip_of_b.branches.contains(&"hypothesis-B".to_string()));
        assert!(graph.nodes.iter().any(|n| n.hash == s1.hash));
        assert!(graph.nodes.iter().any(|n| n.hash == s2.hash));

        // дорожки слияния и родителей существуют в разметке
        assert!(merge_node.lane < graph.lanes_total);
        assert!(!merge_node.lanes_active.is_empty());
    }
}
