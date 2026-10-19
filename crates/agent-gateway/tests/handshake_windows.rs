//! 仅在 Windows 运行：握手文件的 create_new 语义、三字段内容、ACL fail-closed、
//! 清理行为，以及 DACL 内容断言（仅当前用户一条 allow ACE、切断继承）。
//! Unix 侧同等覆盖见 rpc.rs（`#![cfg(unix)]`）；Linux（47）上本文件空编译。
#![cfg(windows)]
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use std::{ffi::c_void, path::Path, sync::Arc};
use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
    IsValidAcl, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

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

// ---------------------------------------------------------------------------
// DACL 内容断言（w2-gw-win-acl 的自动化回归）：读回握手文件的 DACL，校验
// 「仅当前用户（= owner）一条 allow ACE + PROTECTED_DACL 切断继承」。
// 读取/断言路径对齐 参考实现 codex-rs/windows-sandbox-rs/src/acl(.rs/_tests.rs)：
// GetNamedSecurityInfoW 取 DACL（同 add_allow_ace，acl.rs:769-783）/取 owner
// （同 path_owner_matches，acl.rs:305-342）→ IsValidAcl + GetAclInformation +
// GetAce 遍历（同 dacl_has_write_allow_for_sid，acl.rs:361-400）→
// GetSecurityDescriptorControl 验 SE_DACL_PROTECTED（同 acl_tests.rs:127-131）。
// 全程结构化 Win32 断言，不依赖 icacls 本地化文案。
// ---------------------------------------------------------------------------

/// 继承 ACE 的标志位（参考实现 acl.rs:71 同名常量；windows-sys 导出的
/// `INHERITED_ACE` 是 u32 形态，而 `ACE_HEADER.AceFlags` 是 u8，故按参考本地定义）。
const INHERITED_ACE: u8 = 0x10;
/// 允许型 ACE 的类型值（参考实现 acl.rs:72 同名常量；windows-sys 未导出该值）。
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// 期望的唯一授权掩码：读/写/执行/删除，与 src/handshake_windows.rs 的
/// `WRITE_ALLOW_MASK` 同值（参考实现 acl.rs:508-509）。实现侧常量非 pub、
/// 集成测试不可见，按同值重声明。
const WRITE_ALLOW_MASK: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// UTF-16 + NUL 终止（同实现 `to_utf16_nul` / 参考 winutil 的 `to_wide`：
/// GetNamedSecurityInfoW 系 API 的路径入参形态）。
fn to_wide(path: &Path) -> Vec<u16> {
    let mut v: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
    v.push(0);
    v
}

/// 读回 path 的 DACL（对齐参考 add_allow_ace 的命名查询分支，acl.rs:769-783）：
/// `GetNamedSecurityInfoW` + `DACL_SECURITY_INFORMATION`。返回 (DACL, 描述符)——
/// 两者同属一次分配，调用方以 `LocalFree(描述符)` 统一释放。
fn fetch_dacl(path: &Path) -> (*mut ACL, *mut c_void) {
    let mut p_dacl: *mut ACL = std::ptr::null_mut();
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    // Safety: FFI 调用点。`to_wide` 的临时缓冲存活至本语句结束且 NUL 终止；
    // 成功时 p_sd 为 API 的 LocalAlloc 分配，交由调用方 LocalFree。
    let code = unsafe {
        GetNamedSecurityInfoW(
            to_wide(path).as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut p_dacl,
            std::ptr::null_mut(),
            &mut p_sd,
        )
    };
    assert_eq!(
        code,
        ERROR_SUCCESS,
        "GetNamedSecurityInfoW(DACL) failed for {}: {code}",
        path.display()
    );
    (p_dacl, p_sd)
}

/// 读回 path 的 owner SID（对齐参考 path_owner_matches，acl.rs:305-342：
/// `GetNamedSecurityInfoW` + `OWNER_SECURITY_INFORMATION`）。返回 (owner, 描述符)，
/// owner 指向描述符的分配，调用方以 `LocalFree(描述符)` 统一释放。
fn fetch_owner_sid(path: &Path) -> (PSID, *mut c_void) {
    let mut owner: PSID = std::ptr::null_mut();
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    // Safety: FFI 调用点。形态同 fetch_dacl；owner 的生命周期绑定 p_sd 的分配。
    let code = unsafe {
        GetNamedSecurityInfoW(
            to_wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut p_sd,
        )
    };
    assert_eq!(
        code,
        ERROR_SUCCESS,
        "GetNamedSecurityInfoW(OWNER) failed for {}: {code}",
        path.display()
    );
    assert!(!owner.is_null(), "missing owner for {}", path.display());
    (owner, p_sd)
}

/// SID → `S-1-5-…` 字符串：仅用于断言失败时的可读诊断（结构化输出，不走本地化文案）。
fn sid_to_string(psid: PSID) -> String {
    let mut string: PWSTR = std::ptr::null_mut();
    // Safety: FFI 调用点。psid 来自 GetNamedSecurityInfoW 出参或 ACE 内嵌 SID，均为
    // 合法 SID；成功时 string 为 API 分配的 NUL 终止缓冲，随后的读取与释放在本函数内完成。
    if unsafe { ConvertSidToStringSidW(psid, &mut string) } == 0 {
        return format!("<unprintable sid {psid:p}>");
    }
    // Safety: 只读扫描至首个 NUL 界定长度（API 保证 NUL 终止），不越界。
    let len = unsafe {
        let mut n = 0usize;
        while *string.add(n) != 0 {
            n += 1;
        }
        n
    };
    // Safety: 长度已由上一段以 NUL 界定，切片只读不越界。
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string, len) });
    // Safety: 释放 ConvertSidToStringSidW 的 LocalAlloc 分配，仅此一处。
    unsafe { LocalFree(string as HLOCAL) };
    text
}

/// DACL 快照（断言失败时的诊断输出）：逐 ACE 列出 类型/标志/掩码/SID。
/// 遍历形态对齐参考 dacl_has_write_allow_for_sid（acl.rs:361-400）：
/// GetAclInformation(AclSizeInformation) → GetAce → ACE_HEADER → 头部后
/// u32 掩码与内嵌 SID（偏移取法同 acl.rs:204-206/390-393）。
fn describe_dacl(p_dacl: *mut ACL) -> String {
    // Safety: FFI 出参缓冲先置零（edition 2024 下 zeroed 属 unsafe）：全整数结构体，零值有效。
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    // Safety: FFI 调用点。info 为本栈帧出参缓冲，类型/长度与 AclSizeInformation 匹配。
    if unsafe {
        GetAclInformation(
            p_dacl as *const ACL,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return "<unreadable ACL>".to_string();
    }
    let mut parts = vec![format!("ace_count={}", info.AceCount)];
    for i in 0..info.AceCount {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        // Safety: FFI 调用点。p_ace 为出参；失败时保持 null，走诊断占位分支。
        if unsafe { GetAce(p_dacl as *const ACL, i, &mut p_ace) } == 0 {
            parts.push(format!("#{i}=<GetAce failed>"));
            continue;
        }
        // Safety: p_ace 指向 DACL 分配内第 i 条 ACE 的头部前缀，只读。
        let hdr = unsafe { &*(p_ace as *const ACE_HEADER) };
        // ACE 公共布局：ACE_HEADER + Mask(u32) + SidStart（偏移同 acl.rs:204-206）。
        let sid_ptr = (p_ace as usize
            + std::mem::size_of::<ACE_HEADER>()
            + std::mem::size_of::<u32>()) as PSID;
        // Safety: 各 ACE 类型的头部之后都是 u32 掩码（ALLOWED/DENIED 同构），只读。
        let mask = unsafe { (*(p_ace as *const ACCESS_ALLOWED_ACE)).Mask };
        parts.push(format!(
            "#{} type={} flags={:#04x} mask={:#010x} sid={}",
            i,
            hdr.AceType,
            hdr.AceFlags,
            mask,
            sid_to_string(sid_ptr)
        ));
    }
    parts.join("; ")
}

/// DACL 内容：起网关后读回握手文件的 DACL，断言「仅当前用户」收紧确实生效——
/// DACL 存在且恰好 1 条 ACE；唯一 ACE 为 allow 型、掩码覆盖读/写/执行/删除；
/// trustee SID 与文件 owner SID 相等（EqualSid）；ACE 无继承位，且描述符控制位含
/// SE_DACL_PROTECTED（父目录继承被切断）。此前该结论仅靠主理人真机 icacls 人工
/// 验证（`<user>:(M)`），本用例固化为结构化回归。
#[tokio::test]
async fn handshake_dacl_allows_only_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let g = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();

    let (p_dacl, p_sd) = fetch_dacl(&path);
    let (owner, p_owner_sd) = fetch_owner_sid(&path);
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // Safety: FFI 调用点。p_sd 为 fetch_dacl 所得描述符（本函数末尾统一 LocalFree），
    // control/revision 是本栈帧出参缓冲，用法同 acl_tests.rs:127-131。
    let control_read = unsafe { GetSecurityDescriptorControl(p_sd, &mut control, &mut revision) };
    assert_ne!(
        control_read,
        0,
        "GetSecurityDescriptorControl failed for {}",
        path.display()
    );

    assert!(
        !p_dacl.is_null(),
        "handshake DACL must exist for {}",
        path.display()
    );
    // Safety: FFI 调用点。p_dacl 只读且非空（上一断言保证）；同 acl.rs:272 的
    // IsValidAcl 失败分支，无效 DACL 直接判负。
    assert_ne!(
        unsafe { IsValidAcl(p_dacl) },
        0,
        "invalid DACL for {}",
        path.display()
    );
    // Safety: FFI 出参缓冲先置零（edition 2024 下 zeroed 属 unsafe）：全整数结构体，零值有效。
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    // Safety: FFI 调用点。info 为本栈帧出参缓冲，类型/长度与 AclSizeInformation 匹配。
    let acl_info_read = unsafe {
        GetAclInformation(
            p_dacl as *const ACL,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    assert_ne!(
        acl_info_read,
        0,
        "GetAclInformation failed for {}",
        path.display()
    );
    let dacl_dump = || describe_dacl(p_dacl);
    assert_eq!(
        info.AceCount,
        1,
        "handshake DACL must contain exactly one ACE: {}",
        dacl_dump()
    );

    let mut p_ace: *mut c_void = std::ptr::null_mut();
    // Safety: FFI 调用点。p_ace 为出参；索引 0 由 AceCount==1 界定，随后只读。
    assert_ne!(
        unsafe { GetAce(p_dacl as *const ACL, 0, &mut p_ace) },
        0,
        "GetAce(0) failed for {}",
        path.display()
    );
    // Safety: p_ace 指向 DACL 分配内第 0 条 ACE 的头部前缀，只读。
    let hdr = unsafe { &*(p_ace as *const ACE_HEADER) };
    assert_eq!(
        hdr.AceType,
        ACCESS_ALLOWED_ACE_TYPE,
        "the single ACE must be ACCESS_ALLOWED: {}",
        dacl_dump()
    );
    assert_eq!(
        hdr.AceFlags & INHERITED_ACE,
        0,
        "the single ACE must not carry the inherited flag: {}",
        dacl_dump()
    );
    // ACE 公共布局：ACE_HEADER + Mask(u32) + SidStart（偏移取法同 acl.rs:204-206）。
    let sid_ptr =
        (p_ace as usize + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>()) as PSID;
    // Safety: p_ace 按 ACCESS_ALLOWED_ACE 前缀只读（类型已断言为 allow 型）。
    let ace = unsafe { &*(p_ace as *const ACCESS_ALLOWED_ACE) };
    assert_eq!(
        ace.Mask & WRITE_ALLOW_MASK,
        WRITE_ALLOW_MASK,
        "the allow ACE mask {:#010x} must cover read/write/execute/delete: {}",
        ace.Mask,
        dacl_dump()
    );
    let owner_sid = sid_to_string(owner);
    let ace_sid = sid_to_string(sid_ptr);
    // Safety: FFI 调用点。两个入参均为合法 SID（owner 来自 GetNamedSecurityInfoW、
    // sid_ptr 为 ACE 内嵌 SID），EqualSid 只读比较。
    assert_ne!(
        unsafe { EqualSid(sid_ptr, owner) },
        0,
        "the ACE trustee must equal the file owner (current user): ace={ace_sid} owner={owner_sid}"
    );
    assert_ne!(
        control & SE_DACL_PROTECTED,
        0,
        "SE_DACL_PROTECTED must be set (inheritance cut): {}",
        dacl_dump()
    );

    // Safety: 释放两处 GetNamedSecurityInfoW 的 LocalAlloc 分配，各仅此一次；
    // owner/DACL 指针均指向各自描述符的分配，此后不再解引用。
    unsafe {
        LocalFree(p_sd as HLOCAL);
        LocalFree(p_owner_sd as HLOCAL);
    }
    g.shutdown().await.unwrap();
}
