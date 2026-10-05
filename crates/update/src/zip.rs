//! A small zip reader for the Windows archive: the central directory (zip64 included), stored and deflated entries,
//! CRC-32 checked. No encryption, no other methods. Written here because the archives are our own and `flate2` is
//! already in the build; a reader of arbitrary zips would be a dependency.

use std::io::{Read, Seek, SeekFrom};

use crate::error::{Error, Result};

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// One entry of the central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub crc32: u32,
    pub compressed: u64,
    pub uncompressed: u64,
    pub local_header: u64,
    /// The Unix mode when the entry was made on Unix (`version made by` high byte 3), else `None`.
    pub mode: Option<u32>,
    pub encrypted: bool,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn bad(m: impl Into<String>) -> Error {
    Error::archive(m)
}

/// Read the central directory of the zip in `r`.
pub fn entries<R: Read + Seek>(r: &mut R) -> Result<Vec<Entry>> {
    let len = r.seek(SeekFrom::End(0))?;
    // The end record is 22 bytes plus a comment of up to 65535 bytes.
    let tail_len = len.min(22 + 65_535);
    r.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    r.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(&tail, i) == EOCD_SIG)
        .ok_or_else(|| bad("not a zip file (no end of central directory)"))?;
    let rec = &tail[eocd..];
    let mut count = u64::from(u16_at(rec, 10));
    let mut dir_size = u64::from(u32_at(rec, 12));
    let mut dir_offset = u64::from(u32_at(rec, 16));
    if count == 0xFFFF || dir_size == 0xFFFF_FFFF || dir_offset == 0xFFFF_FFFF {
        // zip64: the locator sits just before the end record and names the zip64 end record.
        let locator_at = (len - tail_len) + eocd as u64;
        if locator_at < 20 {
            return Err(bad("zip64 locator missing"));
        }
        r.seek(SeekFrom::Start(locator_at - 20))?;
        let mut loc = [0u8; 20];
        r.read_exact(&mut loc)?;
        if u32_at(&loc, 0) != EOCD64_LOCATOR_SIG {
            return Err(bad("zip64 locator missing"));
        }
        let eocd64_at = u64_at(&loc, 8);
        r.seek(SeekFrom::Start(eocd64_at))?;
        let mut e64 = [0u8; 56];
        r.read_exact(&mut e64)?;
        if u32_at(&e64, 0) != EOCD64_SIG {
            return Err(bad("zip64 end record missing"));
        }
        count = u64_at(&e64, 32);
        dir_size = u64_at(&e64, 40);
        dir_offset = u64_at(&e64, 48);
    }
    if dir_offset.checked_add(dir_size).is_none_or(|end| end > len) {
        return Err(bad("the central directory lies outside the file"));
    }
    r.seek(SeekFrom::Start(dir_offset))?;
    let mut dir = vec![0u8; dir_size as usize];
    r.read_exact(&mut dir)?;
    let mut entries = Vec::with_capacity(count.min(1 << 20) as usize);
    let mut at = 0usize;
    for _ in 0..count {
        if at + 46 > dir.len() || u32_at(&dir, at) != CENTRAL_SIG {
            return Err(bad("a central directory entry is damaged"));
        }
        let made_by = u16_at(&dir, at + 4);
        let flags = u16_at(&dir, at + 8);
        let method = u16_at(&dir, at + 10);
        let crc32 = u32_at(&dir, at + 16);
        let mut compressed = u64::from(u32_at(&dir, at + 20));
        let mut uncompressed = u64::from(u32_at(&dir, at + 24));
        let name_len = usize::from(u16_at(&dir, at + 28));
        let extra_len = usize::from(u16_at(&dir, at + 30));
        let comment_len = usize::from(u16_at(&dir, at + 32));
        let external = u32_at(&dir, at + 38);
        let mut local_header = u64::from(u32_at(&dir, at + 42));
        let name_at = at + 46;
        let extra_at = name_at + name_len;
        let end = extra_at + extra_len + comment_len;
        if end > dir.len() {
            return Err(bad("a central directory entry is damaged"));
        }
        let name = String::from_utf8_lossy(&dir[name_at..extra_at]).into_owned();
        // The zip64 extra field carries whichever of the three 32-bit fields overflowed, in this order.
        let mut extra = &dir[extra_at..extra_at + extra_len];
        while extra.len() >= 4 {
            let id = u16_at(extra, 0);
            let size = usize::from(u16_at(extra, 2));
            let body = extra
                .get(4..4 + size)
                .ok_or_else(|| bad("a zip64 field is damaged"))?;
            if id == 0x0001 {
                let mut p = 0;
                let mut take = |field: &mut u64, sentinel: u64| -> Result<()> {
                    if *field == sentinel {
                        if p + 8 > body.len() {
                            return Err(bad("a zip64 field is short"));
                        }
                        *field = u64_at(body, p);
                        p += 8;
                    }
                    Ok(())
                };
                take(&mut uncompressed, 0xFFFF_FFFF)?;
                take(&mut compressed, 0xFFFF_FFFF)?;
                take(&mut local_header, 0xFFFF_FFFF)?;
            }
            extra = &extra[4 + size..];
        }
        let mode = ((made_by >> 8) == 3).then_some(external >> 16);
        entries.push(Entry {
            name,
            method,
            crc32,
            compressed,
            uncompressed,
            local_header,
            mode,
            encrypted: flags & 1 != 0,
        });
        at = end;
    }
    Ok(entries)
}

/// Read one entry's bytes through `out`, checking its CRC-32.
pub fn read_entry<R: Read + Seek>(
    r: &mut R,
    entry: &Entry,
    out: &mut dyn std::io::Write,
) -> Result<u64> {
    if entry.encrypted {
        return Err(bad(format!("{} is encrypted", entry.name)));
    }
    r.seek(SeekFrom::Start(entry.local_header))?;
    let mut local = [0u8; 30];
    r.read_exact(&mut local)?;
    if u32_at(&local, 0) != LOCAL_SIG {
        return Err(bad(format!("{}: local header missing", entry.name)));
    }
    let name_len = u64::from(u16_at(&local, 26));
    let extra_len = u64::from(u16_at(&local, 28));
    r.seek(SeekFrom::Current((name_len + extra_len) as i64))?;
    let raw = r.by_ref().take(entry.compressed);
    let mut crc = flate2::Crc::new();
    let mut written = 0u64;
    let mut copy = |reader: &mut dyn Read| -> Result<()> {
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            crc.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            written += n as u64;
        }
        Ok(())
    };
    match entry.method {
        0 => copy(&mut { raw })?,
        8 => copy(&mut flate2::read::DeflateDecoder::new(raw))?,
        m => {
            return Err(bad(format!(
                "{}: compression method {m} is not supported",
                entry.name
            )));
        }
    }
    if written != entry.uncompressed {
        return Err(bad(format!(
            "{}: {written} bytes unpacked, {} expected",
            entry.name, entry.uncompressed
        )));
    }
    if crc.sum() != entry.crc32 {
        return Err(bad(format!("{}: CRC-32 mismatch", entry.name)));
    }
    Ok(written)
}

/// A zip writer for the tests and the packaging check: stored or deflated entries, Unix modes, no zip64.
#[cfg(any(test, feature = "test-support"))]
pub mod write {
    use std::io::Write;

    /// Build a zip in memory.
    #[derive(Default)]
    pub struct Writer {
        out: Vec<u8>,
        central: Vec<u8>,
        count: u16,
    }

    impl Writer {
        pub fn new() -> Self {
            Self::default()
        }

        /// Add `name` with `data`; `deflate` compresses it; `mode` is the Unix mode (made-by Unix when `Some`).
        pub fn add(&mut self, name: &str, data: &[u8], deflate: bool, mode: Option<u32>) {
            let compressed = if deflate {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let mut crc = flate2::Crc::new();
            crc.update(data);
            let crc = crc.sum();
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = self.out.len() as u32;
            let made_by: u16 = if mode.is_some() { 3 << 8 } else { 0 };
            // Local header.
            self.out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            self.out.extend_from_slice(&20u16.to_le_bytes());
            self.out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            self.out.extend_from_slice(&method.to_le_bytes());
            self.out.extend_from_slice(&[0, 0, 0, 0]); // time, date
            self.out.extend_from_slice(&crc.to_le_bytes());
            self.out
                .extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            self.out
                .extend_from_slice(&(data.len() as u32).to_le_bytes());
            self.out
                .extend_from_slice(&(name.len() as u16).to_le_bytes());
            self.out.extend_from_slice(&0u16.to_le_bytes());
            self.out.extend_from_slice(name.as_bytes());
            self.out.extend_from_slice(&compressed);
            // Central directory entry.
            let c = &mut self.central;
            c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            c.extend_from_slice(&made_by.to_le_bytes());
            c.extend_from_slice(&20u16.to_le_bytes());
            c.extend_from_slice(&0x0800u16.to_le_bytes());
            c.extend_from_slice(&method.to_le_bytes());
            c.extend_from_slice(&[0, 0, 0, 0]);
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes()); // extra
            c.extend_from_slice(&0u16.to_le_bytes()); // comment
            c.extend_from_slice(&0u16.to_le_bytes()); // disk
            c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            c.extend_from_slice(&(mode.unwrap_or(0) << 16).to_le_bytes());
            c.extend_from_slice(&offset.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            self.count += 1;
        }

        /// The zip's bytes, with a comment so the end-record scan is exercised.
        pub fn finish(self) -> Vec<u8> {
            let mut out = self.out;
            let dir_offset = out.len() as u32;
            out.extend_from_slice(&self.central);
            let comment = b"eludite";
            out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]); // disks
            out.extend_from_slice(&self.count.to_le_bytes());
            out.extend_from_slice(&self.count.to_le_bytes());
            out.extend_from_slice(&(self.central.len() as u32).to_le_bytes());
            out.extend_from_slice(&dir_offset.to_le_bytes());
            out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
            out.extend_from_slice(comment);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stored_and_deflated_entries_read_back_with_their_modes() {
        let mut w = write::Writer::new();
        w.add("eludite-0.1.0-windows-x86_64/", b"", false, Some(0o755));
        w.add(
            "eludite-0.1.0-windows-x86_64/eludite.exe",
            &[7u8; 100_000],
            true,
            Some(0o755),
        );
        w.add("eludite-0.1.0-windows-x86_64/README", b"hello", false, None);
        let bytes = w.finish();
        let mut c = Cursor::new(bytes);
        let entries = entries(&mut c).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dir());
        assert_eq!(entries[1].mode, Some(0o755));
        assert_eq!(entries[2].mode, None);
        let mut out = Vec::new();
        assert_eq!(read_entry(&mut c, &entries[1], &mut out).unwrap(), 100_000);
        assert_eq!(out, vec![7u8; 100_000]);
        let mut out = Vec::new();
        read_entry(&mut c, &entries[2], &mut out).unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn a_damaged_entry_fails_its_crc() {
        let mut w = write::Writer::new();
        w.add("a.txt", b"the quick brown fox", false, None);
        let mut bytes = w.finish();
        bytes[36] ^= 0xff; // inside the stored data of the first entry (30-byte header + 5-byte name = 35)
        let mut c = Cursor::new(bytes);
        let entries = entries(&mut c).unwrap();
        let mut out = Vec::new();
        let err = read_entry(&mut c, &entries[0], &mut out).unwrap_err();
        assert!(err.to_string().contains("CRC-32"), "{err}");
        assert!(super::entries(&mut Cursor::new(b"not a zip at all".to_vec())).is_err());
    }
}
