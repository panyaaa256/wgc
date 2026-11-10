//! Chunk decompression (docs/spec/region-format.md "圧縮方式").

use std::io::Read;

use flate2::read::{MultiGzDecoder, ZlibDecoder};

use super::EXTERNAL_FLAG;

const GZIP: u8 = 1;
const ZLIB: u8 = 2;
const NONE: u8 = 3;
const LZ4: u8 = 4;

/// Why a chunk's data cannot be decompressed (content error C-1).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecompressError {
    /// Custom compression (127) or an unknown compression type.
    #[error("unsupported compression type {0}")]
    UnsupportedCompression(u8),
    /// The payload says the data is external, but none was given.
    #[error("external chunk data is missing")]
    MissingExternal,
    #[error("payload is too short")]
    Truncated,
    #[error("compressed data is corrupt")]
    Corrupt,
    #[error("lz4 checksum mismatch")]
    Lz4Checksum,
}

/// Decompresses a chunk.
///
/// `payload` is the length field, compression byte and data as returned by
/// [`super::RegionFile::payload`]. For an external chunk, `external` is the
/// content of its `.mcc` file.
pub fn decompress(payload: &[u8], external: Option<&[u8]>) -> Result<Vec<u8>, DecompressError> {
    if payload.len() < 5 {
        return Err(DecompressError::Truncated);
    }
    let compression = payload[4];
    let (method, data) = if compression >= EXTERNAL_FLAG {
        let data = external.ok_or(DecompressError::MissingExternal)?;
        (compression - EXTERNAL_FLAG, data)
    } else {
        (compression, &payload[5..])
    };
    match method {
        GZIP => read_all(MultiGzDecoder::new(data)),
        ZLIB => read_all(ZlibDecoder::new(data)),
        NONE => Ok(data.to_vec()),
        LZ4 => lz4_block_stream(data),
        _ => Err(DecompressError::UnsupportedCompression(compression)),
    }
}

fn read_all(mut reader: impl Read) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    reader
        .read_to_end(&mut out)
        .map_err(|_| DecompressError::Corrupt)?;
    Ok(out)
}

const LZ4_MAGIC: &[u8; 8] = b"LZ4Block";
const LZ4_HEADER: usize = 21;
const LZ4_METHOD_RAW: u8 = 0x10;
const LZ4_METHOD_LZ4: u8 = 0x20;
const LZ4_SEED: u32 = 0x9747_b28c;
const LZ4_LEVEL_BASE: u32 = 10;

/// Decodes the lz4-java `LZ4BlockOutputStream` format.
fn lz4_block_stream(mut data: &[u8]) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    // A stream that ends cleanly at a block boundary is accepted like a final empty block.
    while !data.is_empty() {
        if data.len() < LZ4_HEADER || &data[..8] != LZ4_MAGIC {
            return Err(DecompressError::Corrupt);
        }
        let token = data[8];
        let method = token & 0xF0;
        let max_block = 1usize << (LZ4_LEVEL_BASE + u32::from(token & 0x0F));
        let compressed_len = le_len(&data[9..13])?;
        let original_len = le_len(&data[13..17])?;
        let checksum = u32::from_le_bytes(data[17..21].try_into().expect("4 bytes"));
        if original_len == 0 {
            if compressed_len != 0 || checksum != 0 {
                return Err(DecompressError::Corrupt);
            }
            return Ok(out);
        }
        if original_len > max_block || data.len() < LZ4_HEADER + compressed_len {
            return Err(DecompressError::Corrupt);
        }
        let block = &data[LZ4_HEADER..LZ4_HEADER + compressed_len];
        let start = out.len();
        match method {
            LZ4_METHOD_RAW => {
                if compressed_len != original_len {
                    return Err(DecompressError::Corrupt);
                }
                out.extend_from_slice(block);
            }
            LZ4_METHOD_LZ4 => {
                out.resize(start + original_len, 0);
                let written = lz4_flex::block::decompress_into(block, &mut out[start..])
                    .map_err(|_| DecompressError::Corrupt)?;
                if written != original_len {
                    return Err(DecompressError::Corrupt);
                }
            }
            _ => return Err(DecompressError::Corrupt),
        }
        let actual = xxhash_rust::xxh32::xxh32(&out[start..], LZ4_SEED) & 0x0FFF_FFFF;
        if actual != checksum {
            return Err(DecompressError::Lz4Checksum);
        }
        data = &data[LZ4_HEADER + compressed_len..];
    }
    Ok(out)
}

fn le_len(bytes: &[u8]) -> Result<usize, DecompressError> {
    let v = i32::from_le_bytes(bytes.try_into().expect("4 bytes"));
    usize::try_from(v).map_err(|_| DecompressError::Corrupt)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::{GzEncoder, ZlibEncoder};

    use super::*;

    fn payload(compression: u8, data: &[u8]) -> Vec<u8> {
        let mut p = (data.len() as u32 + 1).to_be_bytes().to_vec();
        p.push(compression);
        p.extend_from_slice(data);
        p
    }

    /// Same generators as `tests/fixtures/lz4/Gen.java`.
    fn compressible(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| ((i % 251) ^ ((i / 1000) % 7)) as u8)
            .collect()
    }

    fn noise(n: usize) -> Vec<u8> {
        let mut s: u64 = 0x2545_F491_4F6C_DD1D;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (s >> 56) as u8
            })
            .collect()
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/lz4/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(path).unwrap()
    }

    #[test]
    fn gzip_zlib_and_none() {
        let data = compressible(10_000);
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&data).unwrap();
        let mut zl = ZlibEncoder::new(Vec::new(), Compression::default());
        zl.write_all(&data).unwrap();
        assert_eq!(
            decompress(&payload(GZIP, &gz.finish().unwrap()), None).unwrap(),
            data
        );
        assert_eq!(
            decompress(&payload(ZLIB, &zl.finish().unwrap()), None).unwrap(),
            data
        );
        assert_eq!(decompress(&payload(NONE, &data), None).unwrap(), data);
    }

    #[test]
    fn lz4_from_lz4_java() {
        let p = payload(LZ4, &fixture("compressible_100000.lz4java"));
        assert_eq!(decompress(&p, None).unwrap(), compressible(100_000));
        let p = payload(LZ4, &fixture("noise_1000.lz4java"));
        assert_eq!(decompress(&p, None).unwrap(), noise(1000));
    }

    #[test]
    fn lz4_checksum_mismatch() {
        let mut stream = fixture("noise_1000.lz4java");
        stream[LZ4_HEADER] ^= 0xFF; // flip a byte of the raw block
        assert_eq!(
            decompress(&payload(LZ4, &stream), None),
            Err(DecompressError::Lz4Checksum)
        );
    }

    #[test]
    fn lz4_corrupt_header() {
        let mut stream = fixture("noise_1000.lz4java");
        stream[0] = b'X';
        assert_eq!(
            decompress(&payload(LZ4, &stream), None),
            Err(DecompressError::Corrupt)
        );
    }

    #[test]
    fn external_data() {
        let mut zl = ZlibEncoder::new(Vec::new(), Compression::default());
        zl.write_all(b"outside").unwrap();
        let mcc = zl.finish().unwrap();
        let p = payload(ZLIB + EXTERNAL_FLAG, &[]);
        assert_eq!(decompress(&p, Some(&mcc)).unwrap(), b"outside");
        assert_eq!(decompress(&p, None), Err(DecompressError::MissingExternal));
    }

    #[test]
    fn unsupported_and_corrupt() {
        assert_eq!(
            decompress(&payload(127, b"x"), None),
            Err(DecompressError::UnsupportedCompression(127))
        );
        assert_eq!(
            decompress(&payload(9, b"x"), None),
            Err(DecompressError::UnsupportedCompression(9))
        );
        assert_eq!(
            decompress(&payload(ZLIB, b"not zlib"), None),
            Err(DecompressError::Corrupt)
        );
        assert_eq!(
            decompress(&[0, 0, 0], None),
            Err(DecompressError::Truncated)
        );
    }
}
