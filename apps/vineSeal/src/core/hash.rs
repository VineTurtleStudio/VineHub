use md5::Digest;
use std::hash::Hasher as _;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    Sha256,
    Xxh3,
    Crc32,
    Md5,
}

impl HashAlgo {
    pub fn label(self) -> &'static str {
        match self {
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Xxh3 => "xxh3",
            HashAlgo::Crc32 => "crc32",
            HashAlgo::Md5 => "md5",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sha256" | "sha-256" => Some(HashAlgo::Sha256),
            "xxh3" | "xxhash3" => Some(HashAlgo::Xxh3),
            "crc32" | "crc-32" => Some(HashAlgo::Crc32),
            "md5" => Some(HashAlgo::Md5),
            _ => None,
        }
    }

    pub const ALL: [HashAlgo; 4] = [
        HashAlgo::Sha256,
        HashAlgo::Xxh3,
        HashAlgo::Crc32,
        HashAlgo::Md5,
    ];
}

enum Inner {
    Sha256(sha2::Sha256),
    Xxh3(Box<xxhash_rust::xxh3::Xxh3>),
    Crc32(crc32fast::Hasher),
    Md5(md5::Md5),
}

pub struct Hasher {
    inner: Inner,
}

impl Hasher {
    pub fn new(algo: HashAlgo) -> Self {
        let inner = match algo {
            HashAlgo::Sha256 => Inner::Sha256(sha2::Sha256::new()),
            HashAlgo::Xxh3 => Inner::Xxh3(Box::new(xxhash_rust::xxh3::Xxh3::new())),
            HashAlgo::Crc32 => Inner::Crc32(crc32fast::Hasher::new()),
            HashAlgo::Md5 => Inner::Md5(md5::Md5::new()),
        };
        Hasher { inner }
    }

    pub fn update(&mut self, data: &[u8]) {
        match &mut self.inner {
            Inner::Sha256(h) => h.update(data),
            Inner::Xxh3(h) => h.update(data),
            Inner::Crc32(h) => h.update(data),
            Inner::Md5(h) => h.update(data),
        }
    }

    pub fn finalize_hex(self) -> String {
        match self.inner {
            Inner::Sha256(h) => format!("{:x}", h.finalize()),
            Inner::Xxh3(h) => format!("{:016x}", h.finish()),
            Inner::Crc32(h) => format!("{:08x}", h.finalize()),
            Inner::Md5(h) => format!("{:x}", h.finalize()),
        }
    }
}

pub fn hash_bytes(algo: HashAlgo, data: &[u8]) -> String {
    let mut h = Hasher::new(algo);
    h.update(data);
    h.finalize_hex()
}

pub fn hash_reader(algo: HashAlgo, reader: &mut impl Read) -> std::io::Result<String> {
    let mut h = Hasher::new(algo);
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize_hex())
}

pub fn hash_file(algo: HashAlgo, path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(crate::core::paths::to_long_path(path))?;
    hash_reader(algo, &mut f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_abc() {
        assert_eq!(
            hash_bytes(HashAlgo::Sha256, b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn md5_abc() {
        assert_eq!(hash_bytes(HashAlgo::Md5, b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn crc32_check_vector() {
        assert_eq!(hash_bytes(HashAlgo::Crc32, b"123456789"), "cbf43926");
    }

    #[test]
    fn xxh3_empty() {
        assert_eq!(hash_bytes(HashAlgo::Xxh3, b""), "2d06800538d394c2");
    }

    #[test]
    fn streaming_matches_bytes() {
        let data = vec![7u8; 300_000];
        for algo in HashAlgo::ALL {
            let mut cur = std::io::Cursor::new(&data);
            assert_eq!(hash_reader(algo, &mut cur).unwrap(), hash_bytes(algo, &data));
        }
    }

    #[test]
    fn parse_labels() {
        assert_eq!(HashAlgo::parse("SHA-256"), Some(HashAlgo::Sha256));
        assert_eq!(HashAlgo::parse("xxhash3"), Some(HashAlgo::Xxh3));
        assert_eq!(HashAlgo::parse("nope"), None);
    }
}
