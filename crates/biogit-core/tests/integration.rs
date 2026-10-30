//! Интеграционный тест: полный цикл эксперимента от первого снимка до слияния
//! гипотез и «машины времени».

use biogit_core::{merge_branch, Repo};
use serde_json::json;

const CSV_BASE: &str = "gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\n";
const CSV_MAIN: &str = "gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\nMYC,3.2,9.9,0.012\n";
const CSV_HYP: &str = "gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\nTP53,8.4,8.1,0.77\n";

#[test]
fn полный_цикл_эксперимента() {
    let dir = tempfile::tempdir().expect("временная папка");
    let root = dir.path();
    let repo = Repo::init(root).expect("инициализация");

    // --- 1. Первый снимок: сырьё + рецепт ---
    std::fs::write(root.join("expression.csv"), CSV_BASE).expect("файл");
    std::fs::write(root.join("reads.fasta"), ">r1\nATCGATCGNNNATCG\n").expect("файл");
    let params = json!({"temperature": 310, "ph": 7.4, "iterations": 500});
    let s1 = repo
        .save_snapshot("Начальные данные", "Дмитрий", &params)
        .expect("снимок");
    assert_eq!(s1.branch, "main");
    assert_eq!(s1.files_count, 2);
    assert!(s1.parent.is_empty());
    assert_eq!(s1.parameters, params);
    assert_eq!(s1.hash_short().len(), 10);

    //Ветвь гипотезы от текущего состояния
    repo.create_branch("hypothesis-low-ph").expect("ветка");

    // Работа продолжается на main 
    std::fs::write(root.join("expression.csv"), CSV_MAIN).expect("файл");
    let s2 = repo
        .save_snapshot("Добавили MYC", "Дмитрий", &params)
        .expect("снимок");
    assert_eq!(s2.parent, vec![s1.hash.clone()]);

    //  Переключение на гипотезу: файлы откатываются 
    repo.switch_branch("hypothesis-low-ph").expect("переключение");
    let csv_on_branch = std::fs::read_to_string(root.join("expression.csv")).expect("содержимое");
    assert_eq!(csv_on_branch, CSV_BASE, "переключение вернуло файлы ветки-основы");

    //  Изменения на гипотезе
    std::fs::write(root.join("expression.csv"), CSV_HYP).expect("файл");
    let s3 = repo
        .save_snapshot("Гипотеза: низкий pH", "Анна", &params)
        .expect("снимок");
    assert_eq!(s3.branch, "hypothesis-low-ph");
    assert_eq!(s3.parent, vec![s1.hash.clone()], "гипотеза растёт от s1");

    // Слияние гипотезы в main 
    repo.switch_branch("main").expect("переключение");
    let report = merge_branch(&repo, "hypothesis-low-ph").expect("слияние");
    assert!(report.conflicts.is_empty(), "конфликты: {:?}", report.conflicts);
    let merged = report.snapshot.as_ref().expect("снимок слияния");
    assert_eq!(merged.parent, vec![s2.hash.clone(), s3.hash.clone()]);

    // Итоговый CSV содержит правки обеих веток 
    let final_csv = std::fs::read_to_string(root.join("expression.csv")).expect("итог");
    assert!(final_csv.contains("MYC,3.2,9.9"), "правка main: {final_csv}");
    assert!(final_csv.contains("TP53,8.4,8.1"), "правка гипотезы: {final_csv}");
    assert!(final_csv.contains("BRCA1,10.2,15.7"), "общая строка: {final_csv}");

    //История main: s1 -> s2 -> слияние 
    let history = repo.history("main").expect("история");
    assert_eq!(history.len(), 3, "s1, s2, слияние");
    assert_eq!(history[0].hash, merged.hash);
    assert_eq!(history[0].parent.len(), 2, "у слияния два родителя");

    //Машина времени: откат к первому снимку
    let restored = repo.restore(&s1.hash).expect("восстановление");
    assert_eq!(restored, 2, "expression.csv + reads.fasta");
    let rolled_back = std::fs::read_to_string(root.join("expression.csv")).expect("содержимое");
    assert_eq!(rolled_back, CSV_BASE);
}

#[test]
fn повторное_сохранение_не_дублирует_блобы() {
    let dir = tempfile::tempdir().expect("временная папка");
    let repo = Repo::init(dir.path()).expect("инициализация");
    std::fs::write(dir.path().join("const.fasta"), ">r\nATCGATCGNNNATCG\n").expect("файл");

    let _s1 = repo
        .save_snapshot("первый", "Аня", &serde_json::Value::Null)
        .expect("снимок");
    let objects_dir = dir.path().join(".biogit/objects");
    let count_files = || -> usize {
        std::fs::read_dir(&objects_dir)
            .expect("objects")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|d| std::fs::read_dir(d.path()).expect("подпапка").count())
            .sum()
    };
    let after_first = count_files();

    let _s2 = repo
        .save_snapshot("второй", "Аня", &serde_json::Value::Null)
        .expect("снимок");
    let after_second = count_files();
    assert_eq!(after_second, after_first + 1, "добавился только объект-снимок");
}
