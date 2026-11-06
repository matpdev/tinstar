//! `file://` URIs ⇄ paths.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    let encoded = uri.as_str().strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = encoded.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

pub fn from_path(path: &Path) -> Uri {
    let mut s = String::from("file://");
    for &b in path.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    Uri::from_str(&s).expect("an encoded file path is a valid URI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Path::new("/home/me/a b/çà.ts");
        assert_eq!(to_path(&from_path(p)).unwrap(), p);
    }
}
