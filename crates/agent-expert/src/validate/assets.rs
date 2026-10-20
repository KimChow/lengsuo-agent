//! 资源校验（§17 #19、#21）：技能目录、头像。
//!
//! | # | 检查什么 | 失败码 | 级别 | 本文件函数 |
//! |---|---|---|---|---|
//! | 19 | `skills[]` 每项存在且含 `SKILL.md` | `SKILL_PATH_MISSING` | Error | [`skill_paths_valid`] |
//! | 21 | `avatar`（若声明）存在且 ≤500KB（且尺寸合规） | `AVATAR_MISSING` / `AVATAR_TOO_LARGE` / `AVATAR_DIMENSIONS` | Error | [`avatar_valid`] |
//!
//! **#21 的头像尺寸口径**（本卡要求「24 项中含头像尺寸」）：
//!
//! - 规格 §3 目录结构写「`avatars/`（**512×512**，≤500KB）」，§17 #21 只列了
//!   「存在且 ≤500KB」两条判据——**尺寸判据来自 §3**；
//! - 判定方式：**读图片头**（不装图像库）——PNG 读 `IHDR`、JPEG 读 `SOFn`、
//!   GIF 读逻辑屏宽高；解析不出尺寸 ⇒ 不判尺寸（只报能判的两条）；
//! - 期望尺寸 **512×512**（§3 明写）。两份真实样本的头像实测均为 512×512
//!   （`pandaai-logo-v2.png` 28 315 B、`expert.png` 314 487 B）⇒ 正例通过。
//!
//! **#20 `SCRIPT_SYNTAX` 不在本卡**：见 `validate/mod.rs` 模块文档。

use std::fs;
use std::path::Path;

use crate::error::ExpertError;
use crate::validate::structure::resolve_within;
use crate::validate::ValidateContext;
use crate::validate_errors::ItemError;

/// 头像体积上限：500KB（§17 #21）。
pub(crate) const AVATAR_MAX_BYTES: u64 = 500 * 1024;

/// 头像期望边长：512（§3 目录结构「512×512」）。
pub(crate) const AVATAR_EXPECTED_EDGE: u32 = 512;

/// 技能包标志文件名（§17 #19）。
const SKILL_FILE: &str = "SKILL.md";

/// §17 #19：`skills[]` 每项存在且含 `SKILL.md`。
pub fn skill_paths_valid(ctx: &ValidateContext) -> Result<(), ItemError> {
    for declared in &ctx.manifest.skills {
        let resolved = resolve_within(ctx.dir, declared)?;
        if !resolved.is_dir() {
            return Err(ItemError::Expert(ExpertError::SkillPathMissing {
                path: declared.clone(),
            }));
        }
        if !resolved.join(SKILL_FILE).is_file() {
            return Err(ItemError::Expert(ExpertError::SkillPathMissing {
                path: declared.clone(),
            }));
        }
    }
    Ok(())
}

/// §17 #21：`avatar`（若声明）存在、≤500KB、尺寸 512×512。
pub fn avatar_valid(ctx: &ValidateContext) -> Result<(), ItemError> {
    let Some(declared) = ctx.manifest.avatar.as_ref() else {
        // 未声明头像 ⇒ 本项不判（§17 #21 原文「若声明」）。
        return Ok(());
    };
    let resolved = resolve_within(ctx.dir, declared)?;
    if !resolved.is_file() {
        return Err(ItemError::Expert(ExpertError::AvatarMissing {
            path: declared.clone(),
        }));
    }
    let bytes = fs::metadata(&resolved)
        .map_err(|source| ItemError::Expert(ExpertError::io(&resolved, &source)))?
        .len();
    if bytes > AVATAR_MAX_BYTES {
        return Err(ItemError::Expert(ExpertError::AvatarTooLarge {
            path: declared.clone(),
            bytes,
            limit: AVATAR_MAX_BYTES,
        }));
    }
    if !ctx.rules.strict_avatar_dimensions {
        return Ok(());
    }
    match image_dimensions(&resolved)? {
        Some((width, height))
            if width != AVATAR_EXPECTED_EDGE || height != AVATAR_EXPECTED_EDGE =>
        {
            Err(ItemError::AvatarDimensions {
                path: declared.clone(),
                width,
                height,
                expected_width: AVATAR_EXPECTED_EDGE,
                expected_height: AVATAR_EXPECTED_EDGE,
            })
        }
        _ => Ok(()),
    }
}

/// 读图片头的尺寸（PNG / JPEG / GIF）；不认识的格式返回 `Ok(None)`。
///
/// **只读文件头**（PNG 前 24 B / JPEG 扫 marker / GIF 前 10 B），不装图像库。
pub(crate) fn image_dimensions(path: &Path) -> Result<Option<(u32, u32)>, ItemError> {
    let bytes =
        fs::read(path).map_err(|source| ItemError::Expert(ExpertError::io(path, &source)))?;
    if let Some(size) = png_dimensions(&bytes) {
        return Ok(Some(size));
    }
    if let Some(size) = gif_dimensions(&bytes) {
        return Ok(Some(size));
    }
    Ok(jpeg_dimensions(&bytes))
}

/// PNG 尺寸（`IHDR` 块宽高，各 4 字节大端）。
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((width, height))
}

/// GIF 尺寸（逻辑屏宽高，小端）。
fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || (&bytes[..6] != b"GIF89a" && &bytes[..6] != b"GIF87a") {
        return None;
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    Some((width, height))
}

/// JPEG 尺寸（扫 `SOF0`–`SOF15` marker，跳过 `DHT`/`JPG`/`DAC`）。
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut index = 2usize;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        // 填充字节：0xff 0xff …
        if marker == 0xff {
            index -= 1;
            continue;
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        let is_sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if is_sof {
            let height = u16::from_be_bytes([bytes[index + 3], bytes[index + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            return Some((width, height));
        }
        index += length;
    }
    None
}
