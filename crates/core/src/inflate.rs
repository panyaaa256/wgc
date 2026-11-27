//! zlib and gzip decoding with an output size limit.

use alloc::boxed::Box;
use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::inflate_flags::{
    TINFL_FLAG_PARSE_ZLIB_HEADER, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
};
use miniz_oxide::inflate::core::{DecompressorOxide, decompress};

/// Why a stream could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The stream is malformed, truncated, or fails its checksum.
    Corrupt,
    /// The decoded data would exceed the limit.
    TooLarge,
}

/// Smallest size the output grows by.
pub(crate) const MIN_GROWTH: usize = 64 * 1024;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const GZIP_DEFLATE: u8 = 8;
const GZIP_HEADER: usize = 10;
const GZIP_TRAILER: usize = 8;
const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const FRESERVED: u8 = 0xe0;

/// A reusable inflate state. Allocating it anew for every chunk costs a
/// noticeable share of the time spent reading a world.
pub struct Inflater {
    state: Box<DecompressorOxide>,
}

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Inflater {
    pub fn new() -> Self {
        Self {
            state: Box::default(),
        }
    }

    /// Appends the content of a zlib stream to `out`. Bytes after the end of
    /// the stream are ignored; a stream that ends early or fails its Adler-32
    /// checksum is corrupt.
    pub fn zlib(
        &mut self,
        data: &[u8],
        out: &mut Vec<u8>,
        limit: usize,
    ) -> Result<(), InflateError> {
        self.deflate(data, out, limit, TINFL_FLAG_PARSE_ZLIB_HEADER)
            .map(drop)
    }

    /// Appends the content of a gzip file to `out`. The file is one or more
    /// complete gzip members and nothing else: an empty input, trailing bytes,
    /// reserved header flags, or a wrong header or content checksum are all
    /// corrupt. `limit` applies to the total of all members.
    pub fn gzip(
        &mut self,
        mut data: &[u8],
        out: &mut Vec<u8>,
        limit: usize,
    ) -> Result<(), InflateError> {
        let start = out.len();
        loop {
            let header = gzip_header_len(data)?;
            let member_start = out.len();
            let remaining = limit - (member_start - start);
            let consumed = self.deflate(&data[header..], out, remaining, 0)?;
            let trailer = data
                .get(header + consumed..header + consumed + GZIP_TRAILER)
                .ok_or(InflateError::Corrupt)?;
            let member = &out[member_start..];
            let (crc, size) = trailer.split_at(4);
            // ISIZE is the member size modulo 2^32.
            if le_u32(crc) != Some(crc32fast::hash(member))
                || le_u32(size) != Some(member.len() as u32)
            {
                return Err(InflateError::Corrupt);
            }
            data = &data[header + consumed + GZIP_TRAILER..];
            if data.is_empty() {
                return Ok(());
            }
        }
    }

    /// Appends one deflate stream (raw, or zlib with
    /// [`TINFL_FLAG_PARSE_ZLIB_HEADER`]) to `out` and returns the number of
    /// input bytes it took.
    fn deflate(
        &mut self,
        data: &[u8],
        out: &mut Vec<u8>,
        limit: usize,
        flags: u32,
    ) -> Result<usize, InflateError> {
        let flags = flags | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        let start = out.len();
        // Back references may only reach into this stream's own output, so the
        // decoder sees the output from `start` on.
        let cap = limit.saturating_add(1);
        let mut produced = 0;
        let mut consumed = 0;
        self.state.init();
        loop {
            if out.len() == start + produced {
                let grow = produced.max(MIN_GROWTH).min(cap - produced);
                out.resize(start + produced + grow, 0);
            }
            let (status, read, written) = decompress(
                &mut self.state,
                &data[consumed..],
                &mut out[start..],
                produced,
                flags,
            );
            consumed += read;
            produced += written;
            match status {
                TINFLStatus::Done => break,
                TINFLStatus::HasMoreOutput if produced < cap => {}
                TINFLStatus::HasMoreOutput => {
                    out.truncate(start + produced);
                    return Err(InflateError::TooLarge);
                }
                _ => {
                    out.truncate(start + produced);
                    return Err(InflateError::Corrupt);
                }
            }
        }
        out.truncate(start + produced);
        if produced > limit {
            return Err(InflateError::TooLarge);
        }
        Ok(consumed)
    }
}

/// Length of the gzip member header at the start of `data`, after checking it.
fn gzip_header_len(data: &[u8]) -> Result<usize, InflateError> {
    let fixed = data.get(..GZIP_HEADER).ok_or(InflateError::Corrupt)?;
    let flags = fixed[3];
    if fixed[..2] != GZIP_MAGIC || fixed[2] != GZIP_DEFLATE || flags & FRESERVED != 0 {
        return Err(InflateError::Corrupt);
    }
    let mut len = GZIP_HEADER;
    if flags & FEXTRA != 0 {
        let xlen = data.get(len..len + 2).ok_or(InflateError::Corrupt)?;
        len += 2 + usize::from(u16::from_le_bytes([xlen[0], xlen[1]]));
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            let rest = data.get(len..).ok_or(InflateError::Corrupt)?;
            let nul = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or(InflateError::Corrupt)?;
            len += nul + 1;
        }
    }
    if flags & FHCRC != 0 {
        let stored = data.get(len..len + 2).ok_or(InflateError::Corrupt)?;
        // The header checksum is the low 16 bits of the CRC-32 of the header.
        if u16::from_le_bytes([stored[0], stored[1]]) != crc32fast::hash(&data[..len]) as u16 {
            return Err(InflateError::Corrupt);
        }
        len += 2;
    }
    if len > data.len() {
        return Err(InflateError::Corrupt);
    }
    Ok(len)
}

/// The little-endian `u32` in `bytes`, if it is exactly four bytes long.
pub(crate) fn le_u32(bytes: &[u8]) -> Option<u32> {
    bytes.try_into().ok().map(u32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::write::{GzEncoder, ZlibEncoder};
    use flate2::{Compression, GzBuilder};

    use super::*;

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn gunzip(data: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
        let mut out = Vec::new();
        Inflater::new().gzip(data, &mut out, limit).map(|()| out)
    }

    fn unzlib(data: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
        let mut out = Vec::new();
        Inflater::new().zlib(data, &mut out, limit).map(|()| out)
    }

    #[test]
    fn gzip_members_are_concatenated() {
        let two = [gz(b"hello"), gz(b"world")].concat();
        assert_eq!(gunzip(&two, 100).unwrap(), b"helloworld");
    }

    #[test]
    fn gzip_optional_header_fields_are_skipped() {
        let mut e = GzBuilder::new()
            .filename("level.dat")
            .comment("c")
            .extra(vec![1, 2, 3])
            .write(Vec::new(), Compression::default());
        e.write_all(b"hello").unwrap();
        assert_eq!(gunzip(&e.finish().unwrap(), 100).unwrap(), b"hello");
    }

    #[test]
    fn gzip_header_checksum_is_checked() {
        let plain = gz(b"hi");
        let mut header = plain[..GZIP_HEADER].to_vec();
        header[3] |= FHCRC;
        let crc = crc32fast::hash(&header) as u16;
        let with = |crc: u16| [&header[..], &crc.to_le_bytes(), &plain[GZIP_HEADER..]].concat();
        assert_eq!(gunzip(&with(crc), 100).unwrap(), b"hi");
        assert_eq!(gunzip(&with(crc ^ 1), 100), Err(InflateError::Corrupt));
    }

    #[test]
    fn malformed_gzip_is_corrupt() {
        let one = gz(b"hello");
        let n = one.len();
        let flipped = |i: usize, bit: u8| {
            let mut v = one.clone();
            v[i] ^= bit;
            v
        };
        let cases: [(&str, Vec<u8>); 10] = [
            ("empty", Vec::new()),
            ("trailing zeros", [one.clone(), vec![0; 4]].concat()),
            ("trailing garbage", [one.clone(), b"xyz".to_vec()].concat()),
            ("bad crc", flipped(n - 8, 1)),
            ("bad size", flipped(n - 1, 1)),
            ("truncated trailer", one[..n - 3].to_vec()),
            ("truncated header", one[..5].to_vec()),
            ("name flag without name", flipped(3, FNAME)),
            ("reserved flag", flipped(3, 0x20)),
            ("not deflate", flipped(2, 0x0f)),
        ];
        for (name, data) in cases {
            assert_eq!(gunzip(&data, 100), Err(InflateError::Corrupt), "{name}");
        }
    }

    #[test]
    fn zlib_ignores_trailing_bytes_and_checks_adler32() {
        let z = zlib(b"hello");
        assert_eq!(
            unzlib(&[z.clone(), b"tail".to_vec()].concat(), 100).unwrap(),
            b"hello"
        );
        let mut bad = z.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(unzlib(&bad, 100), Err(InflateError::Corrupt));
        assert_eq!(unzlib(&z[..z.len() - 6], 100), Err(InflateError::Corrupt));
    }

    #[test]
    fn limit_is_inclusive() {
        let data = vec![7u8; 200_000];
        assert_eq!(unzlib(&zlib(&data), 200_000).unwrap(), data);
        assert_eq!(unzlib(&zlib(&data), 199_999), Err(InflateError::TooLarge));
        assert_eq!(gunzip(&gz(&data), 200_000).unwrap(), data);
        let two = [gz(&data), gz(&data)].concat();
        assert_eq!(gunzip(&two, 399_999), Err(InflateError::TooLarge));
    }

    #[test]
    fn appends_after_existing_output() {
        let mut out = b"prefix".to_vec();
        Inflater::new().zlib(&zlib(b"data"), &mut out, 4).unwrap();
        assert_eq!(out, b"prefixdata");
    }
}
