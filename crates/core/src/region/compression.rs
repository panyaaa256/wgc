//! Chunk decompression.

use std::io::Read;

use flate2::read::MultiGzDecoder;
use flate2::{Decompress, FlushDecompress, Status};

use super::EXTERNAL_FLAG;

const GZIP: u8 = 1;
const ZLIB: u8 = 2;
const NONE: u8 = 3;
const LZ4: u8 = 4;

/// Largest decompressed chunk accepted. Larger output is treated as corrupt
/// so that a hostile or broken `.mcc` cannot exhaust memory.
pub const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;

/// Why a chunk's data cannot be decompressed (content error `C-1`).
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
    #[error("decompressed data exceeds {MAX_DECOMPRESSED_BYTES} bytes")]
    TooLarge,
    #[error("lz4 checksum mismatch")]
    Lz4Checksum,
}

/// Decompresses a chunk.
///
/// `payload` is the length field, compression byte and data as returned by
/// [`super::RegionFile::payload`]. For an external chunk, `external` is the
/// content of its `.mcc` file.
///
/// To decompress many chunks, use one [`Decompressor`] for all of them.
pub fn decompress(payload: &[u8], external: Option<&[u8]>) -> Result<Vec<u8>, DecompressError> {
    let mut decompressor = Decompressor::new();
    decompressor.decompress(payload, external)?;
    Ok(decompressor.out)
}

/// Decompresses chunks one after another, reusing the inflate state and the
/// output buffer. Nearly all the time of reading a world goes to inflating
/// chunks, and allocating both anew for each chunk costs about a quarter of it.
pub struct Decompressor {
    inflate: Decompress,
    out: Vec<u8>,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    pub fn new() -> Self {
        Self {
            inflate: Decompress::new(true),
            out: Vec::new(),
        }
    }

    /// Decompresses a chunk like [`decompress`]. The result is valid until
    /// the next call.
    pub fn decompress(
        &mut self,
        payload: &[u8],
        external: Option<&[u8]>,
    ) -> Result<&[u8], DecompressError> {
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
        let out = &mut self.out;
        out.clear();
        match method {
            GZIP => read_all(MultiGzDecoder::new(data), out, MAX_DECOMPRESSED_BYTES)?,
            ZLIB => inflate_zlib(&mut self.inflate, data, out, MAX_DECOMPRESSED_BYTES)?,
            NONE => out.extend_from_slice(data),
            LZ4 => lz4_block_stream(data, out)?,
            _ => return Err(DecompressError::UnsupportedCompression(method)),
        }
        Ok(out)
    }
}

fn read_all(reader: impl Read, out: &mut Vec<u8>, limit: usize) -> Result<(), DecompressError> {
    reader
        .take(limit as u64 + 1)
        .read_to_end(out)
        .map_err(|_| DecompressError::Corrupt)?;
    if out.len() > limit {
        return Err(DecompressError::TooLarge);
    }
    Ok(())
}

/// Smallest capacity `inflate_zlib` gives the output buffer.
const MIN_INFLATE_CAPACITY: usize = 64 * 1024;

/// Inflates a zlib stream into `out`. Like `flate2::read::ZlibDecoder`, bytes
/// after the end of the stream are ignored and a stream that ends early is
/// corrupt.
fn inflate_zlib(
    inflate: &mut Decompress,
    data: &[u8],
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), DecompressError> {
    inflate.reset(true);
    loop {
        if out.len() > limit {
            return Err(DecompressError::TooLarge);
        }
        if out.len() == out.capacity() {
            out.reserve(out.len().max(MIN_INFLATE_CAPACITY));
        }
        // `total_in` never exceeds `data.len()`, which is a `usize`.
        let consumed = inflate.total_in() as usize;
        let produced = out.len();
        let input = &data[consumed..];
        let flush = if input.is_empty() {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        match inflate.decompress_vec(input, out, flush) {
            Ok(Status::StreamEnd) => break,
            // The buffer had room, so no progress means the input ran out.
            Ok(_) if inflate.total_in() as usize == consumed && out.len() == produced => {
                return Err(DecompressError::Corrupt);
            }
            Ok(_) => {}
            Err(_) => return Err(DecompressError::Corrupt),
        }
    }
    if out.len() > limit {
        return Err(DecompressError::TooLarge);
    }
    Ok(())
}

const LZ4_MAGIC: &[u8; 8] = b"LZ4Block";
const LZ4_HEADER: usize = 21;
const LZ4_METHOD_RAW: u8 = 0x10;
const LZ4_METHOD_LZ4: u8 = 0x20;
const LZ4_SEED: u32 = 0x9747_b28c;
const LZ4_LEVEL_BASE: u32 = 10;

/// Decodes the lz4-java `LZ4BlockOutputStream` format.
fn lz4_block_stream(mut data: &[u8], out: &mut Vec<u8>) -> Result<(), DecompressError> {
    // Like lz4-java's LZ4BlockInputStream, a stream must end with an empty block.
    loop {
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
            return Ok(());
        }
        if original_len > max_block || data.len() < LZ4_HEADER + compressed_len {
            return Err(DecompressError::Corrupt);
        }
        if out.len() + original_len > MAX_DECOMPRESSED_BYTES {
            return Err(DecompressError::TooLarge);
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
    fn lz4_requires_the_end_block() {
        let stream = fixture("noise_1000.lz4java");
        let without_end = &stream[..stream.len() - LZ4_HEADER];
        assert_eq!(
            decompress(&payload(LZ4, without_end), None),
            Err(DecompressError::Corrupt)
        );
    }

    #[test]
    fn oversized_output_is_rejected() {
        let mut zl = ZlibEncoder::new(Vec::new(), Compression::fast());
        zl.write_all(&[0u8; 1001]).unwrap();
        let data = zl.finish().unwrap();
        let inflate = |limit| {
            let mut out = Vec::new();
            inflate_zlib(&mut Decompress::new(true), &data, &mut out, limit).map(|()| out.len())
        };
        assert_eq!(inflate(1000), Err(DecompressError::TooLarge));
        assert_eq!(inflate(1001), Ok(1001));

        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&[0u8; 1001]).unwrap();
        let data = gz.finish().unwrap();
        let gunzip = |limit| {
            let mut out = Vec::new();
            read_all(MultiGzDecoder::new(&data[..]), &mut out, limit).map(|()| out.len())
        };
        assert_eq!(gunzip(1000), Err(DecompressError::TooLarge));
        assert_eq!(gunzip(1001), Ok(1001));
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut zl = ZlibEncoder::new(Vec::new(), Compression::default());
        zl.write_all(data).unwrap();
        zl.finish().unwrap()
    }

    #[test]
    fn zlib_output_larger_than_the_initial_buffer() {
        let data = compressible(5 * MIN_INFLATE_CAPACITY + 123);
        assert_eq!(
            decompress(&payload(ZLIB, &zlib(&data)), None).unwrap(),
            data
        );
        // Incompressible data fills the buffer before the input is used up.
        let data = noise(3 * MIN_INFLATE_CAPACITY);
        assert_eq!(
            decompress(&payload(ZLIB, &zlib(&data)), None).unwrap(),
            data
        );
    }

    #[test]
    fn zlib_stream_ends_and_damage() {
        let data = compressible(200_000);
        let stream = zlib(&data);
        let run = |stream: &[u8]| decompress(&payload(ZLIB, stream), None);

        let mut trailing = stream.clone();
        trailing.extend_from_slice(b"ignored");
        assert_eq!(run(&trailing).unwrap(), data);

        for cut in [0, 2, stream.len() / 2, stream.len() - 4, stream.len() - 1] {
            assert_eq!(run(&stream[..cut]), Err(DecompressError::Corrupt), "{cut}");
        }
        let mut bad_checksum = stream.clone();
        *bad_checksum.last_mut().unwrap() ^= 0xFF;
        assert_eq!(run(&bad_checksum), Err(DecompressError::Corrupt));
        let mut flipped = stream.clone();
        flipped[stream.len() / 2] ^= 0xFF;
        assert_eq!(run(&flipped), Err(DecompressError::Corrupt));
    }

    #[test]
    fn decompressor_is_reusable_after_any_result() {
        let big = compressible(300_000);
        let small = noise(10);
        let mut d = Decompressor::new();
        assert_eq!(
            d.decompress(&payload(ZLIB, &zlib(&big)), None).unwrap(),
            big
        );
        assert_eq!(
            d.decompress(&payload(ZLIB, &zlib(&small)), None).unwrap(),
            small
        );
        let cut = zlib(&big);
        assert_eq!(
            d.decompress(&payload(ZLIB, &cut[..cut.len() / 2]), None),
            Err(DecompressError::Corrupt)
        );
        assert_eq!(d.decompress(&payload(NONE, &small), None).unwrap(), small);
        assert_eq!(
            d.decompress(&payload(ZLIB, &zlib(&big)), None).unwrap(),
            big
        );
    }

    #[test]
    fn external_custom_compression_reports_the_method() {
        assert_eq!(
            decompress(&payload(127 + EXTERNAL_FLAG, b""), Some(b"x")),
            Err(DecompressError::UnsupportedCompression(127))
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
