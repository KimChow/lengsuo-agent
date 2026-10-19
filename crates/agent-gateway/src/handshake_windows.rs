//! Windows 握手文件的私有 ACL（Unix 侧 `0o600` 的对位）。
//!
//! 对齐 参考实现 `codex-rs/windows-sandbox-rs/src/acl.rs` 的 API 族与调用顺序：
//! `GetNamedSecurityInfoW`（取 owner，同 `path_owner_matches`，acl.rs:305-342）
//! → `EXPLICIT_ACCESS_W`/`TRUSTEE_W` 组装（`SET_ACCESS` + `TRUSTEE_IS_SID`，同 acl.rs:793-799）
//! → `SetEntriesInAclW` → `SetNamedSecurityInfoW`（同 acl.rs:806-816）。
//! 目标语义是「**只限当前用户**」：旧 DACL 传 null 从零重建（只含一条当前用户 allow ACE），
//! 并带 `PROTECTED_DACL_SECURITY_INFORMATION` 切断父目录继承（写法同 参考实现
//! `setup_provisioning.rs:419-435` 的 protected 分支）——否则继承的 SYSTEM/Users ACE
//! 会让收紧失效。
//!
//! 编译分工（同 agent-sandbox `windows` 模块 / `construct.rs` 的做法）：
//! 纯构造（JSON 内容、UTF-16 转换）不依赖 windows-sys，Linux（47）上可单测；
//! FFI 部分（windows-sys）仅 `cfg(windows)` 编译，其中 unsafe 仅限 WinAPI 调用点。

/// 握手文件内容：`{"port":…,"pid":…,"token":…}`（与 server.rs Unix 分支逐字段一致）。
pub(crate) fn handshake_json(port: u16, pid: u32, token: &str) -> String {
    serde_json::json!({"port":port,"pid":pid,"token":token}).to_string()
}

/// UTF-16 + NUL 终止（参考实现 winutil 的 `to_wide`；同 agent-sandbox `construct.rs`）。
pub(crate) fn to_utf16_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

// ---------------------------------------------------------------------------
// 以下为 windows-sys FFI，仅 `cfg(windows)` 编译（门控同 agent-sandbox/src/windows/acl.rs）。
// ---------------------------------------------------------------------------

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

/// 当前用户的唯一授权掩码：读/写/执行/删除（参考实现 acl.rs:508-509 `WRITE_ALLOW_MASK`；
/// 网关自身后续要读回并删除握手文件，故必须含 `DELETE`）。
#[cfg(windows)]
const WRITE_ALLOW_MASK: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// 同 参考实现 acl.rs:91-97 `acl_api_result`：非 `ERROR_SUCCESS` → 错误信息
/// `"{operation} failed for {path}: {code}"`（anyhow → io::Error 直译，保持消息形状）。
#[cfg(windows)]
fn acl_api_result(path: &Path, operation: &str, code: u32) -> std::io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{operation} failed for {}: {code}",
            path.display()
        )))
    }
}

/// Windows 分支的 `write_handshake` 主体（server.rs 只留薄分发）。
///
/// 流程与 Unix 分支同构：`create_new`（Windows 上映射 `CreationDisposition=CREATE_NEW`，
/// 已存在即失败，绝不覆盖别人的文件）→ 写入三字段 JSON → `sync_all` → 收紧 ACL。
/// 任一步失败都删除已创建的文件并返回错误（fail-closed，绝不留下无保护文件）。
#[cfg(windows)]
pub(crate) fn write_private_handshake(path: &Path, port: u16, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    // The host must supply an existing private directory. Never replace another process's file.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let content = handshake_json(port, std::process::id(), token);
    if let Err(e) = file
        .write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    drop(file);
    if let Err(e) = restrict_to_current_user(path) {
        // fail-closed：ACL 未能收紧就必须删掉文件，绝不留下无保护的 {port,pid,token}。
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// 把 `path` 的 DACL 收紧为「仅当前用户（= 文件 owner = 创建者）一条 allow ACE」。
///
/// 调用顺序对齐 参考实现 acl.rs：`GetNamedSecurityInfoW`(OWNER) 取 owner SID →
/// `SetEntriesInAclW`（旧 DACL 传 null，从零重建）→ `SetNamedSecurityInfoW`
/// （`DACL | PROTECTED_DACL`，切断继承）。owner 取自文件本身而非进程令牌，
/// 与 `path_owner_matches`（acl.rs:305-342）的取法一致：Elevated 下 owner 可能是
/// Administrators 组，此时授权对象即创建进程的有效身份，语义仍是「当前用户」。
#[cfg(windows)]
fn restrict_to_current_user(path: &Path) -> std::io::Result<()> {
    let wide = to_utf16_nul(&path.to_string_lossy());
    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    // Safety: windows-sys FFI。`wide` 是本函数持有的 NUL 终止缓冲；owner SID 指向
    // descriptor 的分配，释放前不再解引用（SetEntriesInAclW 会拷贝受托人 SID）。
    let code = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != ERROR_SUCCESS {
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as HLOCAL) };
        }
        return Err(std::io::Error::other(format!(
            "GetNamedSecurityInfoW failed for {}: {code}",
            path.display()
        )));
    }
    if owner.is_null() {
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as HLOCAL) };
        }
        return Err(std::io::Error::other(format!(
            "missing owner for {}",
            path.display()
        )));
    }
    // 单条 allow ACE：当前用户全权、叶子文件不带继承位（grfInheritance=0，
    // 同 acl.rs:1137 对非目录对象的写法）。Trustee 组装逐字段同 acl.rs:793-799。
    let explicit = EXPLICIT_ACCESS_W {
        grfAccessPermissions: WRITE_ALLOW_MASK,
        grfAccessMode: SET_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: owner.cast(),
        },
    };
    // 旧 DACL 传 null：重建只含这一条 ACE 的 DACL（merge 会保留其他 trustee 的显式
    // ACE，「只限当前用户」就不成立了）。
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    // Safety: windows-sys FFI。explicit 按值借用本栈帧；成功时 new_dacl 由
    // SetEntriesInAclW 新分配（LocalFree 释放），受托人 SID 已被拷贝进去。
    let code = unsafe { SetEntriesInAclW(1, &explicit, std::ptr::null_mut(), &mut new_dacl) };
    if code != ERROR_SUCCESS || new_dacl.is_null() {
        unsafe { LocalFree(descriptor as HLOCAL) };
        return Err(std::io::Error::other(format!(
            "SetEntriesInAclW failed for {}: {code}",
            path.display()
        )));
    }
    // PROTECTED_DACL：切断父目录继承，保证生效 DACL 恰为上面这一条
    //（同 参考实现 setup_provisioning.rs:430-435 的 DaclInheritance::Protected）。
    // Safety: windows-sys FFI。按路径写入（字符串形态与 Get 侧同一缓冲）；new_dacl
    // 在调用返回后统一释放。
    let code = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        )
    };
    let result = acl_api_result(path, "SetNamedSecurityInfoW", code);
    // Safety: 释放本函数两处 LocalAlloc 所得（ACL 与安全描述符），仅在此处释放一次。
    unsafe {
        LocalFree(new_dacl as HLOCAL);
        LocalFree(descriptor as HLOCAL);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯构造：三字段齐全、值精确、无多余字段（与 Unix 分支同一形状）。
    #[test]
    fn handshake_json_has_exactly_port_pid_token() {
        let s = handshake_json(65000, 4242, "abc");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["port"], 65000);
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["token"], "abc");
        assert_eq!(v.as_object().unwrap().len(), 3);
    }

    /// 纯构造：UTF-16 + NUL 终止（FFI 侧 Get/SetNamedSecurityInfoW 的路径入参形态）。
    #[test]
    fn utf16_is_nul_terminated() {
        let text = r"C:\dir\gateway.json";
        let w = to_utf16_nul(text);
        assert_eq!(w.last(), Some(&0));
        assert_eq!(w.len(), text.len() + 1);
        assert_eq!(&w[..3], &[b'C' as u16, b':' as u16, b'\\' as u16]);
    }

    /// ACL 失败路径（单元级）：对不存在的路径收紧 DACL 必须报错
    /// （GetNamedSecurityInfoW 失败 → Err；上层 write_private_handshake 据此删文件）。
    #[cfg(windows)]
    #[test]
    fn restrict_missing_path_is_error() {
        let missing = std::env::temp_dir().join("agent-gateway-no-such-handshake.json");
        let _ = std::fs::remove_file(&missing);
        assert!(restrict_to_current_user(&missing).is_err());
    }
}
