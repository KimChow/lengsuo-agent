//! 仅在 Windows 运行：握手文件的 create_new 语义、三字段内容、ACL fail-closed、
//! 清理行为。Unix 侧同等覆盖见 rpc.rs（`#![cfg(unix)]`）；Linux（47）上本文件空编译。
#![cfg(windows)]
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

fn runtime(dir: &Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

/// 握手文件创建成功：三字段（port/pid/token）与网关实际值一致；正常关停后按 token 匹配删除。
#[tokio::test]
async fn handshake_file_contains_port_pid_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let g = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["port"], g.port());
    assert_eq!(v["pid"], std::process::id());
    assert_eq!(v["token"], g.token());
    g.shutdown().await.unwrap();
    assert!(
        !path.exists(),
        "shutdown must remove the matching handshake"
    );
}

/// create_new 语义：文件已存在时第二次起服被拒，且原文件不被覆盖。
#[tokio::test]
async fn second_create_on_existing_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let g = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();
    let second = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await;
    assert!(
        second.is_err(),
        "CREATE_NEW must refuse to replace an existing handshake file"
    );
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["token"], g.token(), "the first gateway's file survives");
    g.shutdown().await.unwrap();
}

/// fail-closed：不可创建的路径 → 起服报错，且不留任何残留文件
/// （ACL 失败同样走「删除已创建文件 + Err」，见 src/handshake_windows.rs 的清理分支）。
#[tokio::test]
async fn uncreatable_path_fails_closed_without_residue() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-dir").join("gateway.json");
    assert!(GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&missing, Isolation::local_default()),
    )
    .await
    .is_err());
    assert!(!missing.exists());
}

/// 清理行为不变：token 不匹配的握手文件不被删除（remove_handshake 校验后才删）。
#[tokio::test]
async fn shutdown_keeps_handshake_when_token_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let g = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["token"] = json!("not-the-gateway-token");
    std::fs::write(&path, v.to_string()).unwrap();
    g.shutdown().await.unwrap();
    assert!(path.exists(), "a mismatched token must not be removed");
    std::fs::remove_file(&path).unwrap();
}
