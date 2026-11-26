//! # BioGit — ядро системы управления версиями экспериментальных данных
//!
//! Ядро реализует:
//! - CAS-хранилище ([`storage`]): объекты адресуются SHA-256 содержимого,
//!   данные сжимаются gzip, файлы читаются стримингово чанками по 64 КБ;
//! - 2-битное кодирование ([`encoding`]): A=00, C=01, G=10, T=11,
//!   позиции N — отдельной битовой маской; обратный комплемент — побитовый NOT;
//! - снимки ([`snapshot`]): файлы рабочей папки + параметры эксперимента
//!   («рецепт») в одном JSON-объекте с проверкой целостности;
//! - ветви гипотез ([`branch`]);
//! - умное слияние ([`merge`]): трёхстороннее по строкам для текста,
//!   по ключевой колонке для CSV (через Polars), отмена при конфликтах.
//!
//!  Пример: 
//!
//! ```
//! use biogit_core::{Repo, Snapshot};
//! use tempfile::tempdir;
//!
//! let dir = tempdir().unwrap();
//! let repo = Repo::init(dir.path()).unwrap();
//! std::fs::write(dir.path().join("expression.csv"), "gene_id,value\nBRCA1,10.2\n").unwrap();
//!
//! let params = serde_json::json!({"temperature": 310, "ph": 7.4});
//! let snap: Snapshot = repo.save_snapshot("Первый снимок", "Ассистент", &params).unwrap();
//! assert_eq!(snap.files_count, 1);
//! ```

pub mod branch;
pub mod diff;
pub mod encoding;
pub mod error;
pub mod excel;
pub mod formula;
pub mod graph;
pub mod merge;
pub mod object;
pub mod snapshot;
pub mod storage;

pub use branch::BranchInfo;
pub use diff::{CsvDiff, DiffLine, FileDiff, FileStatus, working_status, diff_snapshots, diff_working_vs_snapshot};
pub use encoding::{pack_two_bit, reverse_complement, unpack_two_bit, Packed2Bit};
pub use error::{BioGitError, Result};
pub use excel::{
    SheetCell, SheetDoc, SheetGrid, SheetSession, sheet_open, sheet_set_active, sheet_edit,
    sheet_save, sheet_close, sheet_dirty, excel_probe, EditOutcome, ExcelAvailability,
};
pub use formula::evaluate_sheet;
pub use graph::{history_graph, GraphData, GraphNode};
pub use merge::{
    merge_branch, merge_branch_with_options, apply_merge, apply_proposal, propose_merge,
    FileDecision, MergeOptions, MergeProposal, MergeReport, ProposedFile, ConflictDetail,
    ConflictItem,
};
pub use object::{Snapshot, Tree, TreeEntry};
pub use snapshot::{FileContent, FileEntry, Repo};
pub use storage::BlobStore;
