//! Репозиторий BioGit: папка эксперимента + скрытая папка `.biogit`.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{BioGitError, Result};
use crate::object::{Snapshot, Tree, TreeEntry};
use crate::storage::BlobStore;

/// Запись о файле рабочей папки (панель «Сырьё» в интерфейсе).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileEntry {
    /// Относительный путь, разделитель «/».
    pub name: String,
    /// Размер файла, байт.
    pub size: u64,
}

/// Проверяет имя ветки: буквы/цифры/`-`/`_`/`.`, без выхода за пределы refs.
pub(crate) fn validate_branch_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(BioGitError::InvalidBranchName(name.to_string()))
    }
}

/// Репозиторий: корневая папка эксперимента с `.biogit` внутри.
#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
    store: BlobStore,
}

impl Repo {
    /// Создаёт новый репозиторий в указанной папке.
    pub fn init(path: &Path) -> Result<Repo> {
        let biogit = path.join(".biogit");
        if biogit.exists() {
            return Err(BioGitError::AlreadyInitialized(path.to_path_buf()));
        }
        fs::create_dir_all(biogit.join("refs").join("heads"))?;
        fs::write(biogit.join("HEAD"), "main")?;
        let store = BlobStore::new(path)?;
        Ok(Repo {
            root: path.to_path_buf(),
            store,
        })
    }

    /// Открывает существующий репозиторий.
    pub fn open(path: &Path) -> Result<Repo> {
        if !path.join(".biogit").is_dir() {
            return Err(BioGitError::NotARepo(path.to_path_buf()));
        }
        let store = BlobStore::new(path)?;
        Ok(Repo {
            root: path.to_path_buf(),
            store,
        })
    }

    /// Корневая папка эксперимента.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Скрытая папка репозитория (используется модулями branch/merge).
    pub(crate) fn biogit_dir(&self) -> PathBuf {
        self.root.join(".biogit")
    }

    // ссылки и HEAD

    /// Имя текущей ветки (файл `.biogit/HEAD`).
    pub fn head(&self) -> Result<String> {
        let raw = fs::read_to_string(self.biogit_dir().join("HEAD"))?;
        let name = raw.trim().to_string();
        validate_branch_name(&name)?;
        Ok(name)
    }

    fn ref_path(&self, branch: &str) -> Result<PathBuf> {
        validate_branch_name(branch)?;
        Ok(self.biogit_dir().join("refs").join("heads").join(branch))
    }

    pub(crate) fn set_branch_ref(&self, branch: &str, hash: &str) -> Result<()> {
        let path = self.ref_path(branch)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, hash)?;
        Ok(())
    }

    pub(crate) fn read_ref(&self, branch: &str) -> Result<Option<String>> {
        let path = self.ref_path(branch)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(path)?.trim().to_string()))
    }

    /// Снимок на конце указанной ветки.
    pub(crate) fn snapshot_of_branch(&self, branch: &str) -> Result<Snapshot> {
        let hash = self
            .read_ref(branch)?
            .ok_or_else(|| BioGitError::BranchNotFound(branch.to_string()))?;
        self.snapshot(&hash)
    }

    /// Последний снимок текущей ветки (если он есть).
    pub fn current_commit(&self) -> Result<Option<Snapshot>> {
        match self.read_ref(&self.head()?)? {
            Some(hash) => Ok(Some(self.snapshot(&hash)?)),
            None => Ok(None),
        }
    }

    // объекты

    pub(crate) fn store_tree(&self, tree: &Tree) -> Result<String> {
        let bytes = serde_json::to_vec(tree)?;
        self.store.put_bytes(&bytes)
    }

    pub(crate) fn write_commit_object(&self, snapshot: &Snapshot) -> Result<()> {
        let bytes = serde_json::to_vec(snapshot)?;
        self.store.put_bytes_as(&bytes, &snapshot.hash)
    }

    /// Загружает объект-снимок и проверяет целостность его метаданных.
    pub fn snapshot(&self, hash: &str) -> Result<Snapshot> {
        let bytes = self.store.read(hash)?;
        let snap: Snapshot = serde_json::from_slice(&bytes)
            .map_err(|e| BioGitError::CorruptObject(hash.to_string(), e.to_string()))?;
        let recomputed = Snapshot::compute_hash(
            &snap.timestamp,
            &snap.author,
            &snap.message,
            &snap.parent,
            &snap.branch,
            &snap.tree,
            &snap.parameters,
            snap.files_count,
        );
        if recomputed != snap.hash {
            return Err(BioGitError::CorruptObject(
                hash.to_string(),
                "хеш метаданных не совпадает с содержимым".to_string(),
            ));
        }
        Ok(snap)
    }

    /// Загружает дерево файлов снимка.
    pub fn tree_of(&self, snapshot: &Snapshot) -> Result<Tree> {
        let bytes = self.store.read(&snapshot.tree)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| BioGitError::CorruptObject(snapshot.tree.clone(), e.to_string()))
    }

    /// Читает содержимое блоба по хешу.
    pub fn read_blob(&self, hash: &str) -> Result<Vec<u8>> {
        self.store.read(hash)
    }

    /// Прямой доступ к хранилищу (используется модулем слияния).
    pub(crate) fn store(&self) -> &BlobStore {
        &self.store
    }

    // снимки

    /// Сохраняет снимок: файлы рабочей папки + параметры эксперимента.
    pub fn save_snapshot(
        &self,
        message: &str,
        author: &str,
        parameters: &serde_json::Value,
    ) -> Result<Snapshot> {
        if author.trim().is_empty() {
            return Err(BioGitError::InvalidState("не указан автор снимка".into()));
        }

        // 1. Упаковываем все файлы рабочей папки в хранилище
        let files = self.collect_files()?;
        let mut entries = Vec::with_capacity(files.len());
        for file in &files {
            let (hash, size) = self.store.put_file(&self.root.join(&file.name))?;
            entries.push(TreeEntry {
                name: file.name.clone(),
                hash,
                size,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let tree = Tree { entries };
        let tree_hash = self.store_tree(&tree)?;

        // 2. Метаданные снимка
        let branch = self.head()?;
        let parent = match self.current_commit()? {
            Some(prev) => vec![prev.hash],
            None => Vec::new(),
        };
        let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let files_count = tree.entries.len();
        let hash = Snapshot::compute_hash(
            &timestamp,
            author,
            message,
            &parent,
            &branch,
            &tree_hash,
            parameters,
            files_count,
        );

        // 3. Сохраняем объект-снимок и двигаем ссылку ветки
        let snapshot = Snapshot {
            hash: hash.clone(),
            timestamp,
            author: author.to_string(),
            message: message.to_string(),
            parent,
            branch: branch.clone(),
            tree: tree_hash,
            parameters: parameters.clone(),
            files_count,
        };
        self.write_commit_object(&snapshot)?;
        self.set_branch_ref(&branch, &hash)?;
        Ok(snapshot)
    }

    /// Цепочка снимков ветки от нового к старому (по первому родителю).
    pub fn history(&self, branch: &str) -> Result<Vec<Snapshot>> {
        let branch = if branch.trim().is_empty() {
            self.head()?
        } else {
            branch.to_string()
        };
        let tip = self
            .read_ref(&branch)?
            .ok_or_else(|| BioGitError::BranchNotFound(branch.clone()))?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor: Option<String> = Some(tip);
        while let Some(hash) = cursor {
            if !seen.insert(hash.clone()) {
                break; // защита от зацикливания
            }
            let snap = self.snapshot(&hash)?;
            cursor = snap.parent.first().cloned();
            out.push(snap);
        }
        Ok(out)
    }

    /// «Машина времени»: восстанавливает файлы рабочей папки из снимка.
    /// Возвращает число восстановленных файлов.
    pub fn restore(&self, hash: &str) -> Result<usize> {
        let snap = self.snapshot(hash)?;
        let tree = self.tree_of(&snap)?;
        let mut count = 0usize;
        for entry in &tree.entries {
            Self::validate_entry_name(&entry.name)?;
            let target = self.root.join(&entry.name);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = self.store.read(&entry.hash)?;
            fs::write(&target, &data)?;
            count += 1;
        }
        Ok(count)
    }

    /// Список файлов рабочей папки (без скрытых папок, включая `.biogit`).
    pub fn list_files(&self) -> Result<Vec<FileEntry>> {
        self.collect_files()
    }

    /// Защита от path traversal при восстановлении файлов.
    fn validate_entry_name(name: &str) -> Result<()> {
        let dangerous = name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains(':')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..");
        if dangerous {
            return Err(BioGitError::UnsafePath(name.to_string()));
        }
        Ok(())
    }

    /// Обход рабочей папки: все файлы, кроме скрытых (`.biogit` и прочих).
    fn collect_files(&self) -> Result<Vec<FileEntry>> {
        let mut out = Vec::new();
        let walker = WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
        for entry in walker {
            let entry = entry.map_err(|e| {
                BioGitError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    format!("обход папки: {e}"),
                ))
            })?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(&self.root).map_err(|e| {
                BioGitError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    format!("относительный путь: {e}"),
                ))
            })?;
            let name = rel.to_string_lossy().replace('\\', "/");
            let size = entry.metadata()?.len();
            out.push(FileEntry { name, size });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
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
    fn init_создаёт_структуру_и_open_её_находит() {
        let (_dir, repo) = repo_in_tmp();
        assert!(repo.root().join(".biogit").is_dir());
        assert!(repo.root().join(".biogit/refs/heads").is_dir());
        assert_eq!(repo.head().expect("HEAD"), "main");
        Repo::open(repo.root()).expect("повторное открытие");
    }

    #[test]
    fn двойная_инициализация_запрещена() {
        let (_dir, repo) = repo_in_tmp();
        let err = Repo::init(repo.root()).expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::AlreadyInitialized(_)));
    }

    #[test]
    fn открытие_обычной_папки_даёт_ошибку() {
        let dir = tempfile::tempdir().expect("временная папка");
        let err = Repo::open(dir.path()).expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::NotARepo(_)));
    }

    #[test]
    fn снимок_фиксирует_файлы_и_параметры() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("expression.csv"), "gene_id,control\nBRCA1,10.2\n")
            .expect("файл");
        std::fs::write(repo.root().join("reads.fasta"), ">r1\nATCGATCGNNNATCG\n").expect("файл");
        let params = serde_json::json!({"temperature": 310, "ph": 7.4, "iterations": 500});

        let snap = repo.save_snapshot("Первый снимок", "Дмитрий", &params).expect("снимок");
        assert_eq!(snap.branch, "main");
        assert_eq!(snap.files_count, 2);
        assert!(snap.parent.is_empty());
        assert_eq!(snap.parameters, params);
        assert!(repo.root().join(".biogit/refs/heads/main").exists());

        // снимок читается обратно с проверкой целостности
        let loaded = repo.snapshot(&snap.hash).expect("загрузка");
        assert_eq!(loaded, snap);
    }

    #[test]
    fn история_и_родительские_связи() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        let s1 = repo.save_snapshot("раз", "Аня", &serde_json::Value::Null).expect("снимок");
        std::fs::write(repo.root().join("a.txt"), "2").expect("файл");
        let s2 = repo.save_snapshot("два", "Аня", &serde_json::Value::Null).expect("снимок");

        assert_eq!(s2.parent, vec![s1.hash.clone()]);
        let hist = repo.history("main").expect("история");
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].hash, s2.hash, "новый снимок первым");
        assert_eq!(hist[1].hash, s1.hash);

        assert!(repo.current_commit().expect("текущий").is_some());
    }

    #[test]
    fn машина_времени_возвращает_старое_содержимое() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("data.csv"), "v1").expect("файл");
        let s1 = repo.save_snapshot("v1", "Аня", &serde_json::Value::Null).expect("снимок");
        std::fs::write(repo.root().join("data.csv"), "v2").expect("файл");
        let s2 = repo.save_snapshot("v2", "Аня", &serde_json::Value::Null).expect("снимок");

        let restored = repo.restore(&s1.hash).expect("восстановление");
        assert_eq!(restored, 1);
        assert_eq!(std::fs::read_to_string(repo.root().join("data.csv")).expect("содержимое"), "v1");

        let _ = s2; // s2 использован для истории
    }

    #[test]
    fn восстановление_вложенных_папок() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::create_dir_all(repo.root().join("results")).expect("папка");
        std::fs::write(repo.root().join("results/out.csv"), "ok").expect("файл");
        let snap = repo.save_snapshot("с результатами", "Аня", &serde_json::Value::Null).expect("снимок");

        std::fs::remove_dir_all(repo.root().join("results")).expect("удаление");
        assert!(!repo.root().join("results/out.csv").exists());

        repo.restore(&snap.hash).expect("восстановление");
        assert_eq!(std::fs::read_to_string(repo.root().join("results/out.csv")).expect("содержимое"), "ok");
    }

    #[test]
    fn защита_от_path_traversal() {
        for bad in ["../evil.txt", "/etc/passwd", "a/../../b", "C:\\temp\\x", "", "a//b", "./x"] {
            let err = Repo::validate_entry_name(bad).expect_err(bad);
            assert!(matches!(err, BioGitError::UnsafePath(_)), "имя {bad:?}");
        }
        Repo::validate_entry_name("results/expression.csv").expect("корректное имя");
        Repo::validate_entry_name("reads.fasta").expect("корректное имя");
    }

    #[test]
    fn валидация_имён_веток() {
        for bad in ["", "a/b", "..", ".hidden", "вет ка"] {
            assert!(validate_branch_name(bad).is_err(), "имя {bad:?} должно быть отклонено");
        }
        for good in ["main", "hypothesis-A", "эксперимент_2", "ph-7.4"] {
            assert!(validate_branch_name(good).is_ok(), "имя {good:?} должно быть принято");
        }
    }

    #[test]
    fn дедупликация_не_дублирует_объекты() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("const.fasta"), ">r\nATCGATCGNNNATCG\n").expect("файл");
        let _s1 = repo.save_snapshot("первый", "Аня", &serde_json::Value::Null).expect("снимок");
        let store = BlobStore::new(repo.root()).expect("хранилище");
        let count_after_first = store.count_objects();

        // тот же файл, новый снимок: должен добавиться только объект-снимок
        let _s2 = repo.save_snapshot("второй", "Аня", &serde_json::Value::Null).expect("снимок");
        assert_eq!(store.count_objects(), count_after_first + 1);
    }

    #[test]
    fn пустой_автор_отклоняется() {
        let (_dir, repo) = repo_in_tmp();
        let err = repo
            .save_snapshot("снимок", "  ", &serde_json::Value::Null)
            .expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::InvalidState(_)));
    }
}
