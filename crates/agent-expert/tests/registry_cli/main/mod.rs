//! 导出行为测试（拆出以控制单文件行数；父模块提供夹具与导入）。

use super::*;
use zip_read::{read_zip, walk_dir};

mod zip_read;

#[test]
fn export_writes_readable_zip_with_exclusions() {
    let tmp = TempRoot::new("export");
    let root = tmp.experts_root();
    let dir = root.join("demo-expert");
    complete_package(&dir, "demo-expert", "1.0.0");
    // 空目录也必须保留（目录条目）。
    std::fs::create_dir_all(dir.join(paths::AVATARS_DIR).join("empty")).unwrap();
    // 必须排除的两种路径（任何层级）。
    std::fs::write(dir.join(paths::REGISTRY_FILE), b"{\"leak\":true}").unwrap();
    std::fs::create_dir_all(dir.join(".in_use")).unwrap();
    std::fs::write(dir.join(".in_use").join("1234"), b"pid").unwrap();

    let mut registry = ExpertRegistry::default();
    register_expert(&mut registry, &dir).unwrap();
    let out_path = tmp.path().join("out").join("demo-expert.zip");
    let summary = export_expert(&registry, "demo-expert", &out_path).unwrap();
    assert_eq!(summary.id, "demo-expert");
    assert_eq!(summary.path, out_path);
    assert!(summary.files >= 2);
    assert!(summary.dirs >= 4);
    let bytes = read(&out_path);
    assert_eq!(summary.bytes, bytes.len() as u64);

    let actual = read_zip(&bytes);
    let mut expected = BTreeMap::new();
    walk_dir(&dir, "", &mut expected);
    assert_eq!(actual, expected, "zip 内容应与包目录一致");
    assert!(!actual.contains_key("registry.json"));
    assert!(!actual.keys().any(|name| name.starts_with(".in_use/")));
    assert!(actual.contains_key("avatars/empty/"), "空目录应保留");
    assert!(actual.contains_key(".lengsuo-plugin/plugin.json"));

    let err = export_expert(&registry, "no-such-expert", &out_path).unwrap_err();
    assert_eq!(err.code(), "IO_ERROR");
}
