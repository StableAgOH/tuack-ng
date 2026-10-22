use crate::prelude::*;
use sha2::{Digest, Sha256};
use std::io::Read;

/// 流式计算 sha256，输出小写十六进制。
pub fn sha256_hex(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
