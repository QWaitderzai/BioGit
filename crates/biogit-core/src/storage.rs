//! Хранилище объектов (content-addressable storage).
//!
//! Ключ объекта — SHA-256 его несжатого содержимого. Данные хранятся
//! сжатыми (gzip) в `.biogit/objects/<2 символа хеша>/<остаток хеша>`.
//! Одинаковое содержимое попадает в хранилище ровно один раз (дедупликация).

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

use crate::error::{BioGitError, Result};

/// Размер чанка для стриминговой обработки файлов.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// CAS-хранилище блобов внутри репозитория.
#[derive(Debug)]
pub struct BlobStore {
    objects_dir: PathBuf,
}

impl BlobStore {
    /// Открывает (создавая при необходимости) хранилище в `<репозиторий>/.biogit/objects`.
    pub fn new(repo_root: &Path) -> Result<Self> {
        let objects_dir = repo_root.join(".biogit").join("objects");
        fs::create_dir_all(&objects_dir)?;
        Ok(Self { objects_dir })
    }

    /// Уникальный путь временного файла внутри хранилища.
    fn temp_path(&self) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.objects_dir
            .join(format!(".tmp-{}-{nanos}", std::process::id()))
    }

    /// Путь объекта по хешу вида `sha256:<hex>`.
    fn path_for_hash(&self, hash: &str) -> PathBuf {
        let hex_part = hash.strip_prefix("sha256:").unwrap_or(hash);
        if hex_part.len() < 3 {
            return self.objects_dir.join(hex_part);
        }
        let (head, tail) = hex_part.split_at(2);
        self.objects_dir.join(head).join(tail)
    }

    /// Кладёт байты в хранилище, возвращает их SHA-256.
    pub fn put_bytes(&self, data: &[u8]) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        let dest = self.path_for_hash(&hash);
        if dest.exists() {
            return Ok(hash); // дедупликация: содержимое уже хранится
        }

        let tmp = self.temp_path();
        let file = fs::File::create(&tmp)?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(data)?;
        encoder.finish()?; // завершает поток gzip и дописывает контрольную сумму
        fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
        fs::rename(&tmp, &dest)?;
        Ok(hash)
    }

    /// Кладёт байты под заранее известным ключом (для объектов-снимков,
    /// чей идентификатор — хеш метаданных, а не хеш файла JSON).
    pub(crate) fn put_bytes_as(&self, data: &[u8], key: &str) -> Result<()> {
        let dest = self.path_for_hash(key);
        if dest.exists() {
            return Ok(());
        }
        let tmp = self.temp_path();
        let file = fs::File::create(&tmp)?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(data)?;
        encoder.finish()?;
        fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
        fs::rename(&tmp, &dest)?;
        Ok(())
    }

    /// Стримингово читает файл чанками по 64 КБ, считает SHA-256, сжимает на лету
    /// и записывает объект. Возвращает `(хеш, размер исходных данных в байтах)`.
    pub fn put_file(&self, path: &Path) -> Result<(String, u64)> {
        let input = fs::File::open(path)?;
        let mut reader = BufReader::with_capacity(CHUNK_SIZE, input);

        let mut hasher = Sha256::new();
        let tmp = self.temp_path();
        let out_file = fs::File::create(&tmp)?;
        let mut encoder = GzEncoder::new(out_file, Compression::default());

        let mut buffer = [0u8; CHUNK_SIZE];
        let mut total: u64 = 0;
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            encoder.write_all(&buffer[..n])?;
            total += n as u64;
        }
        encoder.finish()?;

        let hash = format!("sha256:{}", hex::encode(hasher.finalize()));
        let dest = self.path_for_hash(&hash);
        if dest.exists() {
            fs::remove_file(&tmp)?; // дедупликация
        } else {
            fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
            fs::rename(&tmp, &dest)?;
        }
        Ok((hash, total))
    }

    /// Есть ли объект с таким хешем в хранилище.
    pub fn has(&self, hash: &str) -> bool {
        self.path_for_hash(hash).exists()
    }

    /// Читает и распаковывает объект по хешу.
    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.path_for_hash(hash);
        if !path.exists() {
            return Err(BioGitError::ObjectNotFound(hash.to_string()));
        }
        let file = fs::File::open(&path)?;
        let mut decoder = GzDecoder::new(file);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| BioGitError::CorruptObject(hash.to_string(), e.to_string()))?;
        Ok(out)
    }

    /// Число объектов в хранилище (диагностика и тесты).
    pub fn count_objects(&self) -> usize {
        walkdir::WalkDir::new(&self.objects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in_tmp() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().expect("временная папка");
        let store = BlobStore::new(dir.path()).expect("хранилище");
        (dir, store)
    }

    #[test]
    fn put_read_roundtrip() {
        let (_dir, store) = store_in_tmp();
        let data = b"ATCGATCGNNNATCG".repeat(100);
        let hash = store.put_bytes(&data).expect("put");
        assert!(hash.starts_with("sha256:"));
        assert!(store.has(&hash));
        assert_eq!(store.read(&hash).expect("read"), data);
    }

    #[test]
    fn дедупликация_по_хешу() {
        let (_dir, store) = store_in_tmp();
        let h1 = store.put_bytes("одинаковое содержимое".as_bytes()).expect("put 1");
        let h2 = store.put_bytes("одинаковое содержимое".as_bytes()).expect("put 2");
        assert_eq!(h1, h2);
        assert_eq!(store.count_objects(), 1);
    }

    #[test]
    fn файл_раскладывается_по_двум_символам_хеша() {
        let (_dir, store) = store_in_tmp();
        let (hash, size) = store
            .put_file(Path::new("Cargo.toml"))
            .expect("файл Cargo.toml существует при запуске тестов крейта");
        assert_eq!(size, std::fs::metadata("Cargo.toml").expect("meta").len());
        let hex_part = hash.strip_prefix("sha256:").expect("префикс");
        let path = store.objects_dir.join(&hex_part[..2]).join(&hex_part[2..]);
        assert!(path.exists(), "объект должен лежать в objects/ab/cdef…");
        assert_eq!(store.read(&hash).expect("read"), std::fs::read("Cargo.toml").expect("src"));
    }

    #[test]
    fn несжатые_данные_меньше_исходных_при_повторах() {
        let (_dir, store) = store_in_tmp();
        let data = vec![b'A'; 10_000];
        let hash = store.put_bytes(&data).expect("put");
        let raw = std::fs::read(store.path_for_hash(&hash)).expect("raw");
        assert!(raw.len() < data.len(), "сжатие должно уменьшать объём");
    }

    #[test]
    fn чтение_несуществующего_объекта_даёт_ошибку() {
        let (_dir, store) = store_in_tmp();
        let err = store
            .read("sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::ObjectNotFound(_)));
    }
}
