//! base64（RFC 4648 の標準アルファベット、`=` で埋める）。MCP の画像は base64 で返す。
//!
//! 依存を足さないための自前（20 行ほど）。Python の `base64` で復号して PNG のシグネチャと
//! 大きさを確かめる検査が `tools/mcp_smoke.py` にある（別の実装での確認）。

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// バイト列を base64 の文字列にする。
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]);
        // 6 ビットずつ取り出す。値は 0..64 なので添字は範囲内。
        let sextet = |shift: usize| char::from(ALPHABET[(n >> shift) & 0x3f]);
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10 のテストベクタ。
    #[test]
    fn rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(input.as_bytes()), expected, "{input:?}");
        }
    }

    /// 全バイト値（+ と / が出る並びを含む）。
    #[test]
    fn all_byte_values() {
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(encode(&[0x00, 0x10, 0x83]), "ABCD");
        let all: Vec<u8> = (0..=255).collect();
        let s = encode(&all);
        assert_eq!(s.len(), 344);
        assert!(s.starts_with("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIj"));
        assert!(s.ends_with("+fr7/P3+/w=="));
    }
}
