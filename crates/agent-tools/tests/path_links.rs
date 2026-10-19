//! 路径安全闸的符号链接面：链接指向禁区、越出工作区、断链与别名根。
//!
//! 链接与假凭据都建在临时工作区里（`Sandbox`），不触碰真实家目录。

use std::fs;

use agent_tools::{FsToolbox, ToolError};

mod support;

use support::Sandbox;

/// 工作区里的假凭据目录 + 一个同名前缀的合法兄弟目录。
fn creds(sb: &Sandbox) {
    sb.put(".ssh/id_rsa", "FAKE-PRIVATE-KEY");
    sb.put(".gnupg/secring.gpg", "FAKE-SECRING");
    sb.put("normal.txt", "plain");
}

#[cfg(unix)]
#[test]
fn 符号链接指向禁区与越出工作区一律被拒() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);
    fs::write(sb.outside.join("secret.txt"), "outside-secret").unwrap();
    fs::create_dir_all(sb.ws("real/sub")).unwrap();
    fs::write(sb.ws("real/sub/inner.txt"), "inner").unwrap();

    std::os::unix::fs::symlink(sb.ws(".ssh"), sb.ws("to_creds")).unwrap();
    std::os::unix::fs::symlink(sb.ws(".gnupg"), sb.ws("to_gnupg")).unwrap();
    std::os::unix::fs::symlink(&sb.outside, sb.ws("to_outside")).unwrap();
    // 指向工作区内普通目录的链接是正向用例，必须照常放行。
    std::os::unix::fs::symlink(sb.ws("real"), sb.ws("to_real")).unwrap();

    // 经链接解析后的目标落在禁区 ⇒ 拒。
    assert!(matches!(
        tb.read("to_creds/id_rsa", 4096).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.get_info("to_creds").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.list_dir("to_gnupg", 0, 10).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.write("to_creds/authorized_keys", "evil", false)
            .unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.remove("to_gnupg/secring.gpg").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(!tb.exists("to_creds/id_rsa"));

    // 经链接越出工作区 ⇒ 所有入口一致拒绝。
    let outside_file = sb.outside.join("secret.txt");
    assert!(matches!(
        tb.read("to_outside/secret.txt", 4096).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.get_info("to_outside/secret.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.list_dir("to_outside", 0, 10).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.write("to_outside/evil.txt", "evil", false).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.make_dir("to_outside/evil-dir").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.remove("to_outside/secret.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.rename("normal.txt", "to_outside/moved.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.rename("to_outside/secret.txt", "moved.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.search("to_outside", "*", 10).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.glob("to_outside", "*", 10).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(!tb.exists("to_outside/secret.txt"));

    // 递归搜索不跟随链接：链接本身不入结果，更不会递归进禁区/区外。
    let (hits, _) = tb.search(".", "*", 100).unwrap();
    let rels: Vec<&str> = hits.iter().map(|h| h.rel_path.as_str()).collect();
    assert!(
        !rels
            .iter()
            .any(|p| p.starts_with("to_outside") || p.starts_with("to_creds")),
        "{rels:?}"
    );
    assert!(
        !rels.iter().any(|p| p.contains("secret.txt")),
        "不得经链接看到区外文件: {rels:?}"
    );

    // 正向：链接指向工作区内普通目录照常可用。
    assert_eq!(tb.read("to_real/sub/inner.txt", 4096).unwrap(), "inner");
    assert!(tb.exists("to_real/sub"));

    // 区外文件与凭据始终未被改动。
    assert_eq!(fs::read_to_string(&outside_file).unwrap(), "outside-secret");
    assert_eq!(sb.get(".ssh/id_rsa"), "FAKE-PRIVATE-KEY");
    assert!(!sb.outside.join("evil.txt").exists());
    assert!(!sb.outside.join("evil-dir").exists());
    assert!(!sb.outside.join("moved.txt").exists());
}

#[cfg(unix)]
#[test]
fn 断链符号链接不成为逃逸通道() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    // 指向区外**不存在**路径的链接：词法前缀仍在工作区内，解析后落到区外。
    let ghost = sb.outside.join("ghost");
    std::os::unix::fs::symlink(&ghost, sb.ws("dangling")).unwrap();

    assert!(matches!(
        tb.make_dir("dangling/new").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.write("dangling/new.txt", "x", false).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(!tb.exists("dangling/new"));
    assert!(!ghost.exists(), "不得经断链在区外创建目录");

    // 指向禁区的不存在子路径同样拒。
    std::os::unix::fs::symlink(sb.ws(".ssh"), sb.ws("to_creds")).unwrap();
    assert!(matches!(
        tb.make_dir("to_creds/extra").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(!sb.ws(".ssh/extra").exists());
}

#[cfg(unix)]
#[test]
fn 工作区经符号链接别名访问时仍按真实根判定() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);
    // 用链接别名当作工作区根构造：解析后仍是同一真实目录，禁区照常拒。
    let alias = sb.outside.join("ws-alias");
    std::os::unix::fs::symlink(&sb.workspace, &alias).unwrap();
    let via_alias = FsToolbox::new(&alias);

    assert_eq!(via_alias.read("normal.txt", 4096).unwrap(), "plain");
    assert!(matches!(
        via_alias.read(".ssh/id_rsa", 4096).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        via_alias.list_dir(".ssh", 0, 10).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert_eq!(
        via_alias.read("normal.txt", 4096).unwrap(),
        tb.read("normal.txt", 4096).unwrap()
    );
}
