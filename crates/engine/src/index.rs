use crate::error::EngineError;

fn hash_from_hex(hex: &str) -> Result<[u8; 32], EngineError> {
    let hash =
        blake3::Hash::from_hex(hex).map_err(|_| EngineError::InvalidHash(hex.to_string()))?;

    Ok(*hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 올바른_hex는_32바이트로_바뀐다() {
        let hex = "00".repeat(31) + "ff"; // 00 31번 + ff
        let bytes = hash_from_hex(&hex).unwrap();
        assert_eq!(bytes[0], 0);
        assert_eq!(bytes[31], 255);
    }

    #[test]
    fn 길이가_틀리거나_hex가_아니면_에러() {
        assert!(hash_from_hex("abc").is_err()); // 너무 짧음
        assert!(hash_from_hex(&"zz".repeat(32)).is_err()); // 64글자지만 hex가 아님
    }
}
