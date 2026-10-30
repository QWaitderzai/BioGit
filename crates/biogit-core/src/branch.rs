//! Ветви гипотез: создание, список, переключение.

use std::fs;

use serde::Serialize;

use crate::error::{BioGitError, Result};
use crate::object::Snapshot;
use crate::snapshot::Repo;

/// Сводка по ветке для интерфейса.
#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    /// Число снимков в цепочке первого родителя.
    pub snapshots: usize,
    /// Сообщение последнего снимка.
    pub latest_message: Option<String>,
    /// Хеш последнего снимка.
    pub latest_hash: Option<String>,
}

impl Repo {
    /// Создаёт ветвь гипотезы от текущего состояния текущей ветки.
    pub fn create_branch(&self, name: &str) -> Result<()> {
        crate::snapshot::validate_branch_name(name)?;
        if self.read_ref(name)?.is_some() {
            return Err(BioGitError::BranchExists(name.to_string()));
        }
        let current = self.current_commit()?.ok_or_else(|| {
            BioGitError::InvalidState(
                "нет ни одного снимка — сначала сохраните снимок на текущей ветке".to_string(),
            )
        })?;
        self.set_branch_ref(name, &current.hash)
    }

    /// Список всех ветвей, отсортированный по имени.
    pub fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let heads = self.biogit_dir().join("refs").join("heads");
        let mut names: Vec<String> = Vec::new();
        if heads.is_dir() {
            for entry in fs::read_dir(&heads)? {
                let entry = entry?;
                if entry.path().is_file() {
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        names.sort();

        let mut out = Vec::with_capacity(names.len());
        for name in names {
            if let Some(hash) = self.read_ref(&name)? {
                let tip: Snapshot = self.snapshot(&hash)?;
                let snapshots = self.history(&name)?.len();
                out.push(BranchInfo {
                    name,
                    snapshots,
                    latest_message: Some(tip.message),
                    latest_hash: Some(hash),
                });
            } else {
                out.push(BranchInfo {
                    name,
                    snapshots: 0,
                    latest_message: None,
                    latest_hash: None,
                });
            }
        }
        Ok(out)
    }

    /// Переключается на ветку и восстанавливает файлы её последнего снимка.
    pub fn switch_branch(&self, name: &str) -> Result<()> {
        crate::snapshot::validate_branch_name(name)?;
        let tip = self
            .read_ref(name)?
            .ok_or_else(|| BioGitError::BranchNotFound(name.to_string()))?;
        self.restore(&tip)?;
        fs::write(self.biogit_dir().join("HEAD"), name)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_in_tmp() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().expect("временная папка");
        let repo = Repo::init(dir.path()).expect("инициализация");
        (dir, repo)
    }

    #[test]
    fn создание_ветки_фиксирует_текущий_снимок() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("data.csv"), "gene_id,control\nBRCA1,10.2\n").expect("файл");
        let snap = repo.save_snapshot("база", "Аня", &serde_json::Value::Null).expect("снимок");

        repo.create_branch("hypothesis-A").expect("ветка");
        let branches = repo.list_branches().expect("список");
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "hypothesis-A"); // сортировка по имени
        assert_eq!(branches[1].name, "main");
        assert_eq!(
            branches[0].latest_hash.as_deref(),
            Some(snap.hash.as_str()),
            "ветка указывает на тот же снимок"
        );
    }

    #[test]
    fn нельзя_создать_ветку_без_снимков() {
        let (_dir, repo) = repo_in_tmp();
        let err = repo.create_branch("orphan").expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::InvalidState(_)));
    }

    #[test]
    fn дубликат_ветки_запрещён() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        let _ = repo.save_snapshot("снимок", "Аня", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-A").expect("первая ветка");
        let err = repo.create_branch("hypothesis-A").expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::BranchExists(_)));
    }

    #[test]
    fn переключение_веток_меняет_файлы_и_head() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("expr.csv"), "main-версия").expect("файл");
        let _ = repo.save_snapshot("main", "Аня", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-B").expect("ветка");

        // смена ветки, затем правки уже на гипотезе
        repo.switch_branch("hypothesis-B").expect("переключение");
        std::fs::write(repo.root().join("expr.csv"), "гипотеза").expect("файл");
        let _ = repo.save_snapshot("на гипотезе", "Аня", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        assert_eq!(repo.head().expect("HEAD"), "main");
        assert_eq!(
            std::fs::read_to_string(repo.root().join("expr.csv")).expect("содержимое"),
            "main-версия",
            "файлы вернулись к состоянию main"
        );

        repo.switch_branch("hypothesis-B").expect("переключение");
        assert_eq!(
            std::fs::read_to_string(repo.root().join("expr.csv")).expect("содержимое"),
            "гипотеза"
        );
    }

    #[test]
    fn переключение_на_несуществующую_ветку() {
        let (_dir, repo) = repo_in_tmp();
        let err = repo.switch_branch("нет-такой").expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::BranchNotFound(_)));
    }
}
