//! 测试用最小 ZIP 读回（纯 Rust，不调外部 `unzip`、不解压到磁盘）。
//!
//! 与被测实现**独立**：CRC-32 在此重新实现一遍，避免同源实现互相掩盖错误。

use std::collections::BTreeMap;
use std::path::Path;

use agent_expert::paths;

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// 独立实现的 CRC-32（IEEE，反射式多项式 `0xEDB88320`）。
fn test_crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// 解析 zip → `{条目名: 内容}`（目录条目内容为空串，名字以 `/` 结尾）。
pub(super) fn read_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&at| u32_at(bytes, at) == 0x0605_4b50)
        .expect("EOCD 缺失");
    let count = u16_at(bytes, eocd + 10) as usize;
    let central_size = u32_at(bytes, eocd + 12) as usize;
    let central_offset = u32_at(bytes, eocd + 16) as usize;
    let mut pos = central_offset;
    let mut out = BTreeMap::new();
    for _ in 0..count {
        assert_eq!(u32_at(bytes, pos), 0x0201_4b50, "中央目录签名错");
        let crc = u32_at(bytes, pos + 16);
        let compressed = u32_at(bytes, pos + 20) as usize;
        let uncompressed = u32_at(bytes, pos + 24) as usize;
        assert_eq!(compressed, uncompressed, "应为 store（不压缩）");
        let name_len = u16_at(bytes, pos + 28) as usize;
        let extra_len = u16_at(bytes, pos + 30) as usize;
        let comment_len = u16_at(bytes, pos + 32) as usize;
        let local_offset = u32_at(bytes, pos + 42) as usize;
        let name = String::from_utf8(bytes[pos + 46..pos + 46 + name_len].to_vec()).unwrap();

        assert_eq!(u32_at(bytes, local_offset), 0x0403_4b50, "本地头签名错");
        assert_eq!(
            u16_at(bytes, local_offset + 6) & 0x0800,
            0x0800,
            "UTF-8 位未置位"
        );
        let local_name = u16_at(bytes, local_offset + 26) as usize;
        let local_extra = u16_at(bytes, local_offset + 28) as usize;
        let start = local_offset + 30 + local_name + local_extra;
        let data = bytes[start..start + compressed].to_vec();
        assert_eq!(u32_at(bytes, local_offset + 14), crc, "本地头 CRC 不一致");
        assert_eq!(test_crc32(&data), crc, "条目 {name} CRC 校验失败");

        out.insert(name, data);
        pos += 46 + name_len + extra_len + comment_len;
    }
    assert_eq!(pos, central_offset + central_size);
    out
}

/// 目录树 → `{条目名: 内容}`（应用与导出相同的排除规则）。
pub(super) fn walk_dir(root: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut children: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|item| item.unwrap().path())
        .collect();
    children.sort();
    for path in children {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name == paths::REGISTRY_FILE || name == ".in_use" {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.file_type().is_symlink() {
            continue;
        }
        let entry_name = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if meta.is_dir() {
            out.insert(format!("{entry_name}/"), Vec::new());
            walk_dir(&path, &entry_name, out);
        } else {
            out.insert(entry_name, std::fs::read(&path).unwrap());
        }
    }
}
