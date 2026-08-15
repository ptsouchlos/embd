//! This module contains helpers for generating SHA256 hashes
//! of files.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};
use std::{fs::File, path::Path};

const HASH_BUFFER_SIZE: usize = 64 * 1024;
/// Bytes inspected at the start of a file to decide whether it's binary,
/// matching the heuristic git itself uses for `core.autocrlf`/diffing.
const BINARY_PROBE_SIZE: usize = 8000;

/// Hash a file's contents with SHA-256, streamed in 64 KiB chunks so multi-GiB
/// vendored blobs don't blow up memory.
///
/// Text files are hashed with CRLF normalized to LF so that `status` doesn't report
/// false drift purely because Windows checkouts convert line endings and other platforms
/// don't. Binary files are hashed byte-for-byte, untouched.
pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(HASH_BUFFER_SIZE, file);
    let is_binary = {
        let probe = reader
            .fill_buf()
            .with_context(|| format!("failed to read {}", path.display()))?;
        probe[..probe.len().min(BINARY_PROBE_SIZE)].contains(&0)
    };

    let mut hasher = Sha256::new();
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    let mut pending_cr = false;
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if n == 0 {
            break;
        }
        if is_binary {
            hasher.update(&buf[..n]);
            continue;
        }

        let mut chunk = &buf[..n];
        if pending_cr {
            if chunk.first() != Some(&b'\n') {
                hasher.update(b"\r");
            }
            pending_cr = false;
        }
        if chunk.last() == Some(&b'\r') {
            pending_cr = true;
            chunk = &chunk[..chunk.len() - 1];
        }
        hasher.update(normalize_crlf(chunk));
    }
    if pending_cr {
        hasher.update(b"\r");
    }
    let digest = hasher.finalize();
    Ok(format!("sha256:{:x}", base16ct::HexDisplay(&digest)))
}

/// Replace every `\r\n` pair with `\n`. Lone `\r` bytes (old Mac-style line
/// endings) are left untouched, matching git's own CRLF normalization.
fn normalize_crlf(chunk: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len());
    let mut i = 0;
    while i < chunk.len() {
        if chunk[i] == b'\r' && chunk.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(chunk[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn hash_file_matches_known_sha256() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data");
        std::fs::write(&path, b"hello world").unwrap();
        let hash = hash_file(&path).unwrap();
        // sha256("hello world") = b94d27b9...
        assert_eq!(
            hash,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hash_file_streams_large_inputs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big");
        // 1 MiB of zeros — exercises the buffer loop without straining memory.
        let data = vec![0u8; 1024 * 1024];
        std::fs::write(&path, &data).unwrap();
        let hash = hash_file(&path).unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
    }

    #[test]
    fn hash_file_normalizes_crlf_to_lf() {
        let dir = tempdir().unwrap();
        let crlf_path = dir.path().join("crlf.txt");
        let lf_path = dir.path().join("lf.txt");
        std::fs::write(&crlf_path, b"alpha\r\nbeta\r\ngamma").unwrap();
        std::fs::write(&lf_path, b"alpha\nbeta\ngamma").unwrap();
        assert_eq!(hash_file(&crlf_path).unwrap(), hash_file(&lf_path).unwrap());
    }

    #[test]
    fn hash_file_preserves_lone_cr() {
        // Bare CR (old Mac-style line endings) is not a CRLF pair and must be
        // left untouched, matching git's own autocrlf semantics.
        let dir = tempdir().unwrap();
        let path = dir.path().join("data");
        std::fs::write(&path, b"alpha\rbeta").unwrap();
        let hash = hash_file(&path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"alpha\rbeta");
        let expected = format!("sha256:{:x}", base16ct::HexDisplay(&hasher.finalize()));
        assert_eq!(hash, expected);
    }

    #[test]
    fn hash_file_does_not_normalize_binary_content() {
        // A NUL byte marks the file as binary; a \r\n later in the same file
        // must survive untouched even though it would be normalized in text.
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.bin");
        let mut data = vec![0u8, 1, 2];
        data.extend_from_slice(b"\r\n");
        std::fs::write(&path, &data).unwrap();
        let hash = hash_file(&path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let expected = format!("sha256:{:x}", base16ct::HexDisplay(&hasher.finalize()));
        assert_eq!(hash, expected);
    }

    #[test]
    fn hash_file_normalizes_crlf_split_across_chunk_boundary() {
        // Places a \r\n exactly on the HASH_BUFFER_SIZE chunk boundary to
        // exercise the pending_cr carry-over logic.
        let dir = tempdir().unwrap();
        let crlf_path = dir.path().join("crlf.txt");
        let lf_path = dir.path().join("lf.txt");

        let mut crlf_data = vec![b'a'; HASH_BUFFER_SIZE - 1];
        crlf_data.push(b'\r');
        crlf_data.push(b'\n');
        crlf_data.extend_from_slice(b"tail");

        let mut lf_data = vec![b'a'; HASH_BUFFER_SIZE - 1];
        lf_data.push(b'\n');
        lf_data.extend_from_slice(b"tail");

        std::fs::write(&crlf_path, &crlf_data).unwrap();
        std::fs::write(&lf_path, &lf_data).unwrap();
        assert_eq!(hash_file(&crlf_path).unwrap(), hash_file(&lf_path).unwrap());
    }
}
