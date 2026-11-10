//! 协议类型由 Rust 生成（T025）：生成的 .ts 入库，测试比对；写盘是 #[ignore] 测试。
//!
//! 三个根：service::Call、service::Response、web::Frame。其余类型靠 visit_dependencies 收齐，
//! 以后新增的线上类型挂在这三个根下就自动生成。

use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ts_rs::{Config, TS, TypeVisitor};

/// 相对 crates/web 的生成目录（web/src/data/generated）。
const GENERATED_DIR: &str = "../../web/src/data/generated";

fn generated_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(GENERATED_DIR)
}

fn config() -> Config {
    Config::new()
        .with_out_dir(generated_dir())
        .with_large_int("number")
}

/// 在内存里生成整棵树：文件路径 → 文件内容。
fn generated_tree() -> BTreeMap<PathBuf, String> {
    let config = config();
    let mut collector = Collector {
        config: &config,
        files: BTreeMap::new(),
        seen: HashSet::new(),
    };
    collect::<service::Call>(&mut collector);
    collect::<service::Response>(&mut collector);
    collect::<web::Frame>(&mut collector);
    collector.files
}

struct Collector<'a> {
    config: &'a Config,
    files: BTreeMap<PathBuf, String>,
    seen: HashSet<TypeId>,
}

impl TypeVisitor for Collector<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        collect::<T>(self);
    }
}

fn collect<T: TS + 'static + ?Sized>(collector: &mut Collector<'_>) {
    if !collector.seen.insert(TypeId::of::<T>()) {
        return;
    }
    let Some(path) = T::output_path() else {
        return;
    };
    let content = T::export_to_string(collector.config)
        .unwrap_or_else(|error| panic!("生成 {} 失败：{error}", path.display()));
    collector.files.insert(path, content);
    T::visit_dependencies(collector);
}

fn read_generated_dir() -> BTreeMap<PathBuf, String> {
    let dir = generated_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        // 目录还没有（第一次生成前）：当成空集合，让文件集合的断言红。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BTreeMap::new();
        }
        Err(error) => panic!("读不到 {}：{error}", dir.display()),
    };
    let mut files = BTreeMap::new();
    for entry in entries {
        let path = entry.expect("读生成目录").path();
        assert!(path.is_file(), "生成目录里出现非文件：{}", path.display());
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| panic!("文件名不是 UTF-8：{}", path.display()));
        let content = fs::read_to_string(&path).expect("读生成文件");
        files.insert(PathBuf::from(name), content.replace("\r\n", "\n"));
    }
    files
}

#[test]
fn generated_types_match() {
    let expected = generated_tree();
    let actual = read_generated_dir();

    let expected_names: Vec<_> = expected.keys().cloned().collect();
    let actual_names: Vec<_> = actual.keys().cloned().collect();
    assert_eq!(
        actual_names, expected_names,
        "web/src/data/generated 的文件集合和 Rust 类型不一致；跑写入命令后重新提交"
    );

    for (path, content) in &expected {
        assert_eq!(
            actual.get(path),
            Some(content),
            "{} 内容和 Rust 类型不一致；跑写入命令后重新提交",
            path.display()
        );
        assert!(
            !content.contains("bigint"),
            "{} 里出现 bigint：u64 要按 number 生成",
            path.display()
        );
    }

    let event = expected
        .get(Path::new("ServiceEvent.ts"))
        .expect("生成树里要有 ServiceEvent.ts");
    assert!(
        event.contains("seq: number"),
        "ServiceEvent.seq 要生成 number：{event}"
    );
    assert!(
        event.contains("cause?:"),
        "ServiceEvent.cause 要生成可选字段：{event}"
    );
}

#[test]
#[ignore = "写 web/src/data/generated；需要更新生成文件时手动跑"]
fn write_generated_types() {
    let dir = generated_dir();
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("清空生成目录");
    }
    fs::create_dir_all(&dir).expect("创建生成目录");
    for (path, content) in generated_tree() {
        let target = dir.join(&path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("创建生成子目录");
        }
        fs::write(&target, content).expect("写生成文件");
    }
}
