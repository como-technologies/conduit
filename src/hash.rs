//! THE sha256-hex helper (follow-up 7): the crate's only touchpoint with
//! `sha2`. Three hand-rolled copies (transcript / store / fake engine) drifted
//! during the spike; a source-walking test below keeps the count at one.

/// Lowercase hex SHA-256 of `bytes`. sha2 0.11's digest output no longer
/// implements `LowerHex`; map bytes explicitly.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent verification once, against the published SHA-256 test
    /// vectors (FIPS 180-2) — every other test in the crate may then use the
    /// helper without becoming circular.
    #[test]
    fn matches_the_published_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Exactly ONE sha256-hex helper exists (done-criterion: grep-checkable):
    /// no file outside this module may touch `sha2` or hand-roll a digest.
    #[test]
    fn sha256_is_single_sourced_in_this_module() {
        let src_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&src_dir, &mut files);
        // Integration tests live outside src/ but must not hand-roll either.
        let tests_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
        if tests_dir.is_dir() {
            walk(&tests_dir, &mut files);
        }
        for f in files {
            if f.file_name().is_some_and(|n| n == "hash.rs") {
                continue;
            }
            let content = std::fs::read_to_string(&f).unwrap();
            if content.contains("Sha256") || content.contains("sha2::") {
                offenders.push(f);
            }
        }
        assert!(
            offenders.is_empty(),
            "sha256 hand-rolled outside src/hash.rs — use crate::hash::sha256_hex: {offenders:?}"
        );
    }
}
