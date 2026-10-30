//! Объекты репозитория: дерево файлов и снимок (аналог коммита).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Запись дерева: один файл снимка.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeEntry {
    /// Относительный путь внутри папки эксперимента, разделитель «/».
    pub name: String,
    /// Хеш содержимого в хранилище (`sha256:…`).
    pub hash: String,
    /// Размер распакованного содержимого, байт.
    pub size: u64,
}

/// Дерево — полный список файлов одного снимка.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

/// Снимок — зафиксированное состояние эксперимента: файлы + параметры.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    /// Хеш снимка (вычисляется по всем полям, кроме самого hash).
    pub hash: String,
    /// Момент создания, RFC 3339 (UTC).
    pub timestamp: String,
    pub author: String,
    pub message: String,
    /// Родительские снимки: один у обычного, два у слияния.
    pub parent: Vec<String>,
    /// Ветка, на которой сделан снимок.
    pub branch: String,
    /// Хеш дерева файлов.
    pub tree: String,
    /// Параметры эксперимента («рецепт») — произвольный JSON.
    #[serde(default)]
    pub parameters: serde_json::Value,
    /// Число файлов в дереве.
    #[serde(default)]
    pub files_count: usize,
}

impl Snapshot {
    /// Короткий хеш для интерфейса: без префикса `sha256:`, первые 10 символов.
    pub fn hash_short(&self) -> String {
        let hex_part = self.hash.strip_prefix("sha256:").unwrap_or(&self.hash);
        hex_part.chars().take(10).collect()
    }

    /// Вычисляет хеш снимка по содержимому его полей.
    ///
    /// Хеш строится по компактному canonical-JSON, поэтому одинаковые
    /// метаданные всегда дают один и тот же идентификатор.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_hash(
        timestamp: &str,
        author: &str,
        message: &str,
        parent: &[String],
        branch: &str,
        tree_hash: &str,
        parameters: &serde_json::Value,
        files_count: usize,
    ) -> String {
        #[derive(Serialize)]
        struct CommitData<'a> {
            timestamp: &'a str,
            author: &'a str,
            message: &'a str,
            parent: &'a [String],
            branch: &'a str,
            tree: &'a str,
            parameters: &'a serde_json::Value,
            files_count: usize,
        }
        let data = CommitData {
            timestamp,
            author,
            message,
            parent,
            branch,
            tree: tree_hash,
            parameters,
            files_count,
        };
        let bytes = match serde_json::to_vec(&data) {
            Ok(bytes) => bytes,
            Err(_) => Vec::new(), // сериализация таких структур не может отказать
        };
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("sha256:{}", hex::encode(hasher.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            hash: String::new(),
            timestamp: "2026-10-29T14:30:00Z".to_string(),
            author: "Дмитрий".to_string(),
            message: "Первый снимок".to_string(),
            parent: Vec::new(),
            branch: "main".to_string(),
            tree: "sha256:abc123".to_string(),
            parameters: serde_json::json!({"temperature": 310, "ph": 7.4, "iterations": 500}),
            files_count: 3,
        }
    }

    #[test]
    fn сериализация_снимка_в_json_и_обратно() {
        let snap = sample_snapshot();
        let json = serde_json::to_string(&snap).expect("сериализация");
        let back: Snapshot = serde_json::from_str(&json).expect("десериализация");
        assert_eq!(back, snap);
    }

    #[test]
    fn хеш_детерминирован_и_чувствителен_к_параметрам() {
        let s = sample_snapshot();
        let h1 = Snapshot::compute_hash(
            &s.timestamp, &s.author, &s.message, &s.parent, &s.branch, &s.tree,
            &s.parameters, s.files_count,
        );
        let h2 = Snapshot::compute_hash(
            &s.timestamp, &s.author, &s.message, &s.parent, &s.branch, &s.tree,
            &s.parameters, s.files_count,
        );
        assert_eq!(h1, h2, "одинаковые данные — одинаковый хеш");
        assert!(h1.starts_with("sha256:"));

        let mut params2 = s.parameters.clone();
        params2["temperature"] = serde_json::json!(298);
        let h3 = Snapshot::compute_hash(
            &s.timestamp, &s.author, &s.message, &s.parent, &s.branch, &s.tree,
            &params2, s.files_count,
        );
        assert_ne!(h1, h3, "изменение параметров меняет хеш");
    }

    #[test]
    fn короткий_хеш_без_префикса() {
        let mut snap = sample_snapshot();
        snap.hash = "sha256:0123456789abcdef".to_string();
        assert_eq!(snap.hash_short(), "0123456789");
    }

    #[test]
    fn сериализация_дерева() {
        let tree = Tree {
            entries: vec![TreeEntry {
                name: "results/expression.csv".to_string(),
                hash: "sha256:deadbeef".to_string(),
                size: 128,
            }],
        };
        let json = serde_json::to_string(&tree).expect("сериализация");
        let back: Tree = serde_json::from_str(&json).expect("десериализация");
        assert_eq!(back, tree);
    }
}
