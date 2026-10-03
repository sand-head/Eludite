//! A minimal portable PDB reader: documents, sequence points and local variable names.
//!
//! Format: "Portable PDB v1.0" (the dotnet/runtime `PortablePdb-Metadata.md` specification) on top of the ECMA-335
//! physical metadata layout (II.24). Only the tables the spike needs are decoded: Document (0x30),
//! MethodDebugInformation (0x31), LocalScope (0x32) and LocalVariable (0x33). Windows (MSF) PDBs are recognized and
//! refused with a clear error. The reader is pure Rust and runs on every OS, so it is unit-tested everywhere.

use std::collections::HashMap;

/// A hidden sequence point's start line (the compiler's "no source" marker).
pub const HIDDEN_LINE: u32 = 0x00fe_efee;

const TABLE_DOCUMENT: usize = 0x30;
const TABLE_METHOD_DEBUG_INFO: usize = 0x31;
const TABLE_LOCAL_SCOPE: usize = 0x32;
const TABLE_LOCAL_VARIABLE: usize = 0x33;
const TABLE_LOCAL_CONSTANT: usize = 0x34;
const TABLE_IMPORT_SCOPE: usize = 0x35;
const TABLE_METHOD_DEF: usize = 0x06;

/// `LocalVariableAttributes.DebuggerHidden`.
const LOCAL_DEBUGGER_HIDDEN: u16 = 0x1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequencePoint {
    pub il_offset: u32,
    /// Row id in the Document table (1-based).
    pub document: u32,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

impl SequencePoint {
    pub fn is_hidden(&self) -> bool {
        self.start_line == HIDDEN_LINE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVariable {
    /// The slot index in the method's local signature.
    pub index: u16,
    pub name: String,
}

#[derive(Debug, Clone)]
struct LocalScope {
    method_row: u32,
    variables: std::ops::Range<u32>,
    start: u32,
    length: u32,
}

/// Where a source line binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineBinding {
    /// The MethodDef token (`0x06xxxxxx`).
    pub method_token: u32,
    pub il_offset: u32,
    /// The sequence point's start line (may be after the requested line).
    pub line: u32,
}

/// A parsed portable PDB.
#[derive(Debug, Clone)]
pub struct PortablePdb {
    documents: Vec<String>,
    /// Indexed by MethodDef row - 1.
    methods: Vec<Vec<SequencePoint>>,
    scopes: Vec<LocalScope>,
    variables: Vec<(u16, u16, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PdbError {
    /// A Windows PDB (MSF container); the spike reads portable PDBs only.
    WindowsPdb,
    Malformed(String),
}

impl std::fmt::Display for PdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdbError::WindowsPdb => write!(f, "a Windows (MSF) PDB; only portable PDBs are read"),
            PdbError::Malformed(m) => write!(f, "malformed portable PDB: {m}"),
        }
    }
}

type R<T> = Result<T, PdbError>;

fn bad<T>(m: impl Into<String>) -> R<T> {
    Err(PdbError::Malformed(m.into()))
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    fn bytes(&mut self, n: usize) -> R<&'a [u8]> {
        if self.remaining() < n {
            return bad("unexpected end of data");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> R<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> R<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn index(&mut self, wide: bool) -> R<u32> {
        if wide {
            self.u32()
        } else {
            Ok(u32::from(self.u16()?))
        }
    }
    /// ECMA-335 II.23.2 compressed unsigned integer.
    fn compressed_u32(&mut self) -> R<(u32, usize)> {
        let b0 = self.u8()?;
        if b0 & 0x80 == 0 {
            Ok((u32::from(b0), 1))
        } else if b0 & 0xc0 == 0x80 {
            let b1 = self.u8()?;
            Ok(((u32::from(b0 & 0x3f) << 8) | u32::from(b1), 2))
        } else if b0 & 0xe0 == 0xc0 {
            let rest = self.bytes(3)?;
            Ok((
                (u32::from(b0 & 0x1f) << 24)
                    | (u32::from(rest[0]) << 16)
                    | (u32::from(rest[1]) << 8)
                    | u32::from(rest[2]),
                4,
            ))
        } else {
            bad("bad compressed integer")
        }
    }
    fn cu(&mut self) -> R<u32> {
        Ok(self.compressed_u32()?.0)
    }
    /// ECMA-335 II.23.2 compressed signed integer (rotated sign bit).
    fn cs(&mut self) -> R<i32> {
        let (raw, len) = self.compressed_u32()?;
        let mut v = (raw >> 1) as i32;
        if raw & 1 != 0 {
            v |= match len {
                1 => 0xffff_ffc0_u32 as i32,
                2 => 0xffff_e000_u32 as i32,
                _ => 0xf000_0000_u32 as i32,
            };
        }
        Ok(v)
    }
}

struct Heaps<'a> {
    strings: &'a [u8],
    blob: &'a [u8],
}

impl<'a> Heaps<'a> {
    fn string(&self, offset: u32) -> R<String> {
        let s = self
            .strings
            .get(offset as usize..)
            .ok_or(PdbError::Malformed("string offset out of range".into()))?;
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    }
    fn blob(&self, offset: u32) -> R<&'a [u8]> {
        let mut c = Cursor::new(
            self.blob
                .get(offset as usize..)
                .ok_or(PdbError::Malformed("blob offset out of range".into()))?,
        );
        let len = c.cu()? as usize;
        c.bytes(len)
    }
    /// A Document.Name blob: a separator byte then blob indexes of UTF-8 parts.
    fn document_name(&self, offset: u32) -> R<String> {
        let mut c = Cursor::new(self.blob(offset)?);
        let sep = c.u8()?;
        let mut out = String::new();
        let mut first = true;
        while c.remaining() > 0 {
            let part = c.cu()?;
            if !first && sep != 0 {
                out.push(char::from(sep));
            }
            first = false;
            if part != 0 {
                out.push_str(&String::from_utf8_lossy(self.blob(part)?));
            }
        }
        Ok(out)
    }
}

impl PortablePdb {
    pub fn parse(data: &[u8]) -> R<Self> {
        if data.starts_with(b"Microsoft C/C++ MSF 7.00") {
            return Err(PdbError::WindowsPdb);
        }
        let mut c = Cursor::new(data);
        if c.u32()? != 0x424a_5342 {
            return bad("no BSJB metadata signature");
        }
        c.u16()?;
        c.u16()?;
        c.u32()?;
        let vlen = c.u32()? as usize;
        c.bytes(vlen)?;
        c.u16()?; // flags
        let nstreams = c.u16()?;
        let mut streams: HashMap<String, &[u8]> = HashMap::new();
        for _ in 0..nstreams {
            let off = c.u32()? as usize;
            let size = c.u32()? as usize;
            let start = c.pos;
            let nul = data[start..]
                .iter()
                .position(|&b| b == 0)
                .ok_or(PdbError::Malformed("unterminated stream name".into()))?;
            let name = String::from_utf8_lossy(&data[start..start + nul]).into_owned();
            let padded = (nul + 1).div_ceil(4) * 4;
            c.bytes(padded)?;
            let body = data
                .get(off..off + size)
                .ok_or(PdbError::Malformed(format!("stream {name} out of range")))?;
            streams.insert(name, body);
        }
        let pdb = streams.get("#Pdb").ok_or(PdbError::Malformed(
            "no #Pdb stream (not a portable PDB)".into(),
        ))?;
        let tables = streams
            .get("#~")
            .ok_or(PdbError::Malformed("no #~ stream".into()))?;
        let heaps = Heaps {
            strings: streams.get("#Strings").copied().unwrap_or(&[]),
            blob: streams.get("#Blob").copied().unwrap_or(&[]),
        };

        // Row counts: type-system tables from #Pdb, debug tables from #~.
        let mut rows = [0u32; 64];
        let mut p = Cursor::new(pdb);
        p.bytes(20)?; // PDB id
        p.u32()?; // entry point
        let referenced = p.u64()?;
        for (t, r) in rows.iter_mut().enumerate() {
            if referenced & (1u64 << t) != 0 {
                *r = p.u32()?;
            }
        }

        let mut t = Cursor::new(tables);
        t.u32()?;
        t.u8()?;
        t.u8()?;
        let heap_sizes = t.u8()?;
        t.u8()?;
        let valid = t.u64()?;
        t.u64()?; // sorted
        for (i, r) in rows.iter_mut().enumerate() {
            if valid & (1u64 << i) != 0 {
                *r = t.u32()?;
            }
        }
        let wide_string = heap_sizes & 0x01 != 0;
        let wide_guid = heap_sizes & 0x02 != 0;
        let wide_blob = heap_sizes & 0x04 != 0;
        let wide = |table: usize| rows[table] > 0xffff;

        // Tables are stored in table-number order; the spike only needs 0x30..=0x33, which come first in a PDB.
        for table in 0..TABLE_DOCUMENT {
            if valid & (1u64 << table) != 0 {
                return bad(format!("type-system table 0x{table:02x} in a PDB"));
            }
        }

        let mut documents = Vec::with_capacity(rows[TABLE_DOCUMENT] as usize);
        for _ in 0..rows[TABLE_DOCUMENT] {
            let name = t.index(wide_blob)?;
            t.index(wide_guid)?; // hash algorithm
            t.index(wide_blob)?; // hash
            t.index(wide_guid)?; // language
            documents.push(heaps.document_name(name)?);
        }

        let mut methods = Vec::with_capacity(rows[TABLE_METHOD_DEBUG_INFO] as usize);
        for _ in 0..rows[TABLE_METHOD_DEBUG_INFO] {
            let doc = t.index(wide(TABLE_DOCUMENT))?;
            let sp = t.index(wide_blob)?;
            let points = if sp == 0 {
                Vec::new()
            } else {
                decode_sequence_points(heaps.blob(sp)?, doc)?
            };
            methods.push(points);
        }

        let mut scopes = Vec::with_capacity(rows[TABLE_LOCAL_SCOPE] as usize);
        let mut raw_scopes = Vec::new();
        for _ in 0..rows[TABLE_LOCAL_SCOPE] {
            let method = t.index(wide(TABLE_METHOD_DEF))?;
            t.index(wide(TABLE_IMPORT_SCOPE))?;
            let var_list = t.index(wide(TABLE_LOCAL_VARIABLE))?;
            t.index(wide(TABLE_LOCAL_CONSTANT))?;
            let start = t.u32()?;
            let length = t.u32()?;
            raw_scopes.push((method, var_list, start, length));
        }
        let nvars = rows[TABLE_LOCAL_VARIABLE];
        for (i, &(method, var_list, start, length)) in raw_scopes.iter().enumerate() {
            // A scope's variables run to the next scope's list start (or the table's end).
            let end = raw_scopes
                .get(i + 1)
                .map(|s| s.1)
                .unwrap_or(nvars + 1)
                .max(var_list);
            scopes.push(LocalScope {
                method_row: method,
                variables: var_list..end,
                start,
                length,
            });
        }

        let mut variables = Vec::with_capacity(nvars as usize);
        for _ in 0..nvars {
            let attrs = t.u16()?;
            let index = t.u16()?;
            let name = t.index(wide_string)?;
            variables.push((attrs, index, heaps.string(name)?));
        }

        Ok(Self {
            documents,
            methods,
            scopes,
            variables,
        })
    }

    pub fn documents(&self) -> &[String] {
        &self.documents
    }

    /// Finds the document matching a client path: the same path (case and separators ignored), else the only
    /// document with that file name. The spike has no source map, so this is the whole of path mapping.
    pub fn find_document(&self, path: &str) -> Option<u32> {
        let norm = |p: &str| p.replace('\\', "/").to_ascii_lowercase();
        let wanted = norm(path);
        if let Some(i) = self.documents.iter().position(|d| norm(d) == wanted) {
            return Some(i as u32 + 1);
        }
        let file = |p: &str| p.rsplit('/').next().unwrap_or("").to_string();
        let wanted_file = file(&wanted);
        let mut matches = self
            .documents
            .iter()
            .enumerate()
            .filter(|(_, d)| file(&norm(d)) == wanted_file);
        match (matches.next(), matches.next()) {
            (Some((i, _)), None) => Some(i as u32 + 1),
            _ => None,
        }
    }

    pub fn document_name(&self, row: u32) -> Option<&str> {
        self.documents
            .get(row.checked_sub(1)? as usize)
            .map(String::as_str)
    }

    pub fn sequence_points(&self, method_token: u32) -> &[SequencePoint] {
        let row = (method_token & 0x00ff_ffff) as usize;
        row.checked_sub(1)
            .and_then(|i| self.methods.get(i))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Binds a source line: the earliest sequence point that starts on `line` in `document`; failing that, the
    /// nearest following line within a method that spans `line`. Prefers the innermost method when several
    /// qualify (lambdas, local functions).
    pub fn bind_line(&self, document: u32, line: u32) -> Option<LineBinding> {
        let mut exact: Option<(u32, LineBinding)> = None;
        let mut next: Option<LineBinding> = None;
        for (row, points) in self.methods.iter().enumerate() {
            let token = 0x0600_0000 | (row as u32 + 1);
            let visible: Vec<&SequencePoint> = points
                .iter()
                .filter(|p| !p.is_hidden() && p.document == document)
                .collect();
            let (Some(first), Some(last)) = (
                visible.iter().map(|p| p.start_line).min(),
                visible.iter().map(|p| p.end_line).max(),
            ) else {
                continue;
            };
            let span = last - first;
            if let Some(p) = visible
                .iter()
                .filter(|p| p.start_line == line)
                .min_by_key(|p| p.il_offset)
            {
                let b = LineBinding {
                    method_token: token,
                    il_offset: p.il_offset,
                    line,
                };
                if exact.is_none_or(|(s, _)| span < s) {
                    exact = Some((span, b));
                }
            } else if first <= line
                && line <= last
                && let Some(p) = visible
                    .iter()
                    .filter(|p| p.start_line > line)
                    .min_by_key(|p| (p.start_line, p.il_offset))
                && next.is_none_or(|n| p.start_line < n.line)
            {
                next = Some(LineBinding {
                    method_token: token,
                    il_offset: p.il_offset,
                    line: p.start_line,
                });
            }
        }
        exact.map(|(_, b)| b).or(next)
    }

    /// The sequence point that covers an IL offset: the last visible one at or before it.
    pub fn point_at(&self, method_token: u32, il_offset: u32) -> Option<SequencePoint> {
        self.sequence_points(method_token)
            .iter()
            .filter(|p| !p.is_hidden() && p.il_offset <= il_offset)
            .max_by_key(|p| p.il_offset)
            .copied()
    }

    /// Named, non-hidden locals in scope at an IL offset, by slot.
    pub fn locals_at(&self, method_token: u32, il_offset: u32) -> Vec<LocalVariable> {
        let row = method_token & 0x00ff_ffff;
        let mut out: Vec<LocalVariable> = Vec::new();
        for s in self.scopes.iter().filter(|s| {
            s.method_row == row
                && s.start <= il_offset
                && il_offset < s.start.saturating_add(s.length)
        }) {
            for v in s.variables.clone() {
                let Some((attrs, index, name)) = self.variables.get(v as usize - 1) else {
                    continue;
                };
                if attrs & LOCAL_DEBUGGER_HIDDEN != 0 || out.iter().any(|l| l.index == *index) {
                    continue;
                }
                out.push(LocalVariable {
                    index: *index,
                    name: name.clone(),
                });
            }
        }
        out.sort_by_key(|l| l.index);
        out
    }
}

/// Decodes a MethodDebugInformation.SequencePoints blob (Portable PDB spec, "Sequence Points Blob").
fn decode_sequence_points(blob: &[u8], initial_document: u32) -> R<Vec<SequencePoint>> {
    let mut c = Cursor::new(blob);
    c.cu()?; // local signature
    let mut document = if initial_document == 0 {
        c.cu()?
    } else {
        initial_document
    };
    let mut out = Vec::new();
    let mut il: u32 = 0;
    let mut prev_line: Option<(u32, u32)> = None;
    let mut first = true;
    while c.remaining() > 0 {
        let delta_il = c.cu()?;
        if !first && delta_il == 0 {
            document = c.cu()?;
            continue;
        }
        il = if first { delta_il } else { il + delta_il };
        first = false;
        let delta_lines = c.cu()?;
        let delta_cols: i64 = if delta_lines == 0 {
            i64::from(c.cu()?)
        } else {
            i64::from(c.cs()?)
        };
        if delta_lines == 0 && delta_cols == 0 {
            out.push(SequencePoint {
                il_offset: il,
                document,
                start_line: HIDDEN_LINE,
                start_column: 0,
                end_line: HIDDEN_LINE,
                end_column: 0,
            });
            continue;
        }
        let (line, col) = match prev_line {
            None => (c.cu()?, c.cu()?),
            Some((pl, pc)) => {
                let dl = c.cs()?;
                let dc = c.cs()?;
                (
                    (i64::from(pl) + i64::from(dl)) as u32,
                    (i64::from(pc) + i64::from(dc)) as u32,
                )
            }
        };
        prev_line = Some((line, col));
        out.push(SequencePoint {
            il_offset: il,
            document,
            start_line: line,
            start_column: col,
            end_line: line + delta_lines,
            end_column: (i64::from(col) + delta_cols) as u32,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compressed unsigned, for building blobs.
    fn cu(v: u32, out: &mut Vec<u8>) {
        if v < 0x80 {
            out.push(v as u8);
        } else if v < 0x4000 {
            out.extend_from_slice(&((v as u16) | 0x8000).to_be_bytes());
        } else {
            out.extend_from_slice(&(v | 0xc000_0000).to_be_bytes());
        }
    }

    /// Compressed signed (ECMA-335 II.23.2), for building blobs.
    fn cs(v: i32, out: &mut Vec<u8>) {
        if (-64..64).contains(&v) {
            let r = ((v << 1) & 0x7e) as u32 | u32::from(v < 0);
            out.push(r as u8);
        } else if (-8192..8192).contains(&v) {
            let r = ((v << 1) & 0x3ffe) as u32 | u32::from(v < 0);
            out.extend_from_slice(&((r as u16) | 0x8000).to_be_bytes());
        } else {
            let r = ((v << 1) & 0x1fff_fffe) as u32 | u32::from(v < 0);
            out.extend_from_slice(&(r | 0xc000_0000).to_be_bytes());
        }
    }

    #[test]
    fn compressed_signed_matches_the_spec_examples() {
        // ECMA-335 II.23.2 examples.
        for (bytes, v) in [
            (&[0x06][..], 3),
            (&[0x7b][..], -3),
            (&[0x80, 0x80][..], 64),
            (&[0x01][..], -64),
            (&[0xc0, 0x00, 0x40, 0x00][..], 8192),
            (&[0x80, 0x01][..], -8192),
            (&[0xdf, 0xff, 0xff, 0xfe][..], 268_435_455),
            (&[0xc0, 0x00, 0x00, 0x01][..], -268_435_456),
        ] {
            assert_eq!(Cursor::new(bytes).cs().unwrap(), v, "{bytes:x?}");
            let mut enc = Vec::new();
            cs(v, &mut enc);
            assert_eq!(enc, bytes, "encoding {v}");
        }
    }

    #[test]
    fn decodes_sequence_points_with_hidden_and_document_switch() {
        let mut b = Vec::new();
        cu(0x11, &mut b); // local signature
        // il 0: line 10 col 5..20
        cu(0, &mut b);
        cu(0, &mut b);
        cu(15, &mut b);
        cu(10, &mut b);
        cu(5, &mut b);
        // il +3: hidden
        cu(3, &mut b);
        cu(0, &mut b);
        cu(0, &mut b);
        // il +4: line 12 (delta +2), col 9 (delta +4), spans 0 lines, 6 columns
        cu(4, &mut b);
        cu(0, &mut b);
        cu(6, &mut b);
        cs(2, &mut b);
        cs(4, &mut b);
        // document switch to 2, then il +2: line 8 (delta -4) col 1 (delta -8), 1 line, -1 columns
        cu(0, &mut b);
        cu(2, &mut b);
        cu(2, &mut b);
        cu(1, &mut b);
        cs(-1, &mut b);
        cs(-4, &mut b);
        cs(-8, &mut b);
        let sps = decode_sequence_points(&b, 1).unwrap();
        assert_eq!(sps.len(), 4);
        assert_eq!(
            sps[0],
            SequencePoint {
                il_offset: 0,
                document: 1,
                start_line: 10,
                start_column: 5,
                end_line: 10,
                end_column: 20
            }
        );
        assert!(sps[1].is_hidden());
        assert_eq!(sps[1].il_offset, 3);
        assert_eq!(
            (sps[2].il_offset, sps[2].start_line, sps[2].start_column),
            (7, 12, 9)
        );
        assert_eq!(sps[2].end_column, 15);
        assert_eq!(
            sps[3],
            SequencePoint {
                il_offset: 9,
                document: 2,
                start_line: 8,
                start_column: 1,
                end_line: 9,
                end_column: 0
            }
        );
    }

    #[test]
    fn refuses_a_windows_pdb_and_garbage() {
        assert_eq!(
            PortablePdb::parse(b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0").unwrap_err(),
            PdbError::WindowsPdb
        );
        assert!(matches!(
            PortablePdb::parse(b"nope").unwrap_err(),
            PdbError::Malformed(_)
        ));
    }

    /// Builds a whole portable PDB image: one document, two methods, one scope with two locals (one hidden).
    fn sample_pdb() -> Vec<u8> {
        // #Strings
        let mut strings = vec![0u8];
        let counter_off = strings.len() as u32;
        strings.extend_from_slice(b"counter\0");
        let hidden_off = strings.len() as u32;
        strings.extend_from_slice(b"CS$1$0000\0");
        while strings.len() % 4 != 0 {
            strings.push(0);
        }
        // #Blob
        let mut blob = vec![0u8];
        let push_blob = |bytes: &[u8], blob: &mut Vec<u8>| -> u32 {
            let off = blob.len() as u32;
            cu(bytes.len() as u32, blob);
            blob.extend_from_slice(bytes);
            off
        };
        let p1 = push_blob(b"C:", &mut blob);
        let p2 = push_blob(b"src", &mut blob);
        let p3 = push_blob(b"Program.cs", &mut blob);
        let mut name = vec![b'\\'];
        cu(p1, &mut name);
        cu(p2, &mut name);
        cu(p3, &mut name);
        let doc_name = push_blob(&name, &mut blob);
        // Method 1 (Main): lines 5 and 6. Method 2 (Tick): lines 10, 11 (il 2 and 7), 12.
        let mut m1 = Vec::new();
        cu(0, &mut m1);
        for (il, line) in [(0u32, 5i32), (1, 6)] {
            cu(il, &mut m1);
            cu(0, &mut m1);
            cu(10, &mut m1);
            if line == 5 {
                cu(5, &mut m1);
                cu(9, &mut m1);
            } else {
                cs(1, &mut m1);
                cs(0, &mut m1);
            }
        }
        let m1 = push_blob(&m1, &mut blob);
        let mut m2 = Vec::new();
        cu(0x11, &mut m2);
        cu(0, &mut m2);
        cu(0, &mut m2);
        cu(2, &mut m2);
        cu(10, &mut m2);
        cu(9, &mut m2);
        cu(2, &mut m2);
        cu(0, &mut m2);
        cu(20, &mut m2);
        cs(1, &mut m2);
        cs(4, &mut m2);
        cu(5, &mut m2);
        cu(0, &mut m2);
        cu(16, &mut m2);
        cs(0, &mut m2);
        cs(0, &mut m2); // a second point on line 11 at il 7
        cu(3, &mut m2);
        cu(0, &mut m2);
        cu(16, &mut m2);
        cs(1, &mut m2);
        cs(-4, &mut m2);
        let m2 = push_blob(&m2, &mut blob);
        while blob.len() % 4 != 0 {
            blob.push(0);
        }
        // #~
        let mut t = Vec::new();
        t.extend_from_slice(&0u32.to_le_bytes());
        t.push(2);
        t.push(0);
        t.push(0); // narrow heaps
        t.push(1);
        let valid: u64 = (1 << 0x30) | (1 << 0x31) | (1 << 0x32) | (1 << 0x33);
        t.extend_from_slice(&valid.to_le_bytes());
        t.extend_from_slice(&0u64.to_le_bytes());
        for n in [1u32, 2, 1, 2] {
            t.extend_from_slice(&n.to_le_bytes());
        }
        // Document: name, hashalg, hash, language
        for v in [doc_name as u16, 0, 0, 0] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        // MethodDebugInformation x2
        for v in [1u16, m1 as u16, 1, m2 as u16] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        // LocalScope: method 2, import 0, varlist 1, constlist 1, start 0, length 20
        for v in [2u16, 0, 1, 1] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&20u32.to_le_bytes());
        // LocalVariable x2
        for (a, i, n) in [(0u16, 0u16, counter_off), (1, 1, hidden_off)] {
            t.extend_from_slice(&a.to_le_bytes());
            t.extend_from_slice(&i.to_le_bytes());
            t.extend_from_slice(&(n as u16).to_le_bytes());
        }
        while t.len() % 4 != 0 {
            t.push(0);
        }
        // #Pdb: id, entry point, referenced tables (MethodDef: 2 rows)
        let mut pdb = vec![0u8; 20];
        pdb.extend_from_slice(&0x0600_0001u32.to_le_bytes());
        pdb.extend_from_slice(&(1u64 << 0x06).to_le_bytes());
        pdb.extend_from_slice(&2u32.to_le_bytes());

        let streams: [(&str, &[u8]); 4] = [
            ("#Pdb", &pdb),
            ("#~", &t),
            ("#Strings", &strings),
            ("#Blob", &blob),
        ];
        let version = b"PDB v1.0\0\0\0\0";
        let mut header_len = 16 + version.len() + 4;
        for (n, _) in &streams {
            header_len += 8 + (n.len() + 1).div_ceil(4) * 4;
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0x424a_5342u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(version.len() as u32).to_le_bytes());
        out.extend_from_slice(version);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(streams.len() as u16).to_le_bytes());
        let mut off = header_len;
        for (n, body) in &streams {
            out.extend_from_slice(&(off as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(n.as_bytes());
            let padded = (n.len() + 1).div_ceil(4) * 4;
            out.extend(std::iter::repeat_n(0, padded - n.len()));
            off += body.len();
        }
        assert_eq!(out.len(), header_len);
        for (_, body) in &streams {
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn reads_documents_lines_and_locals() {
        let pdb = PortablePdb::parse(&sample_pdb()).unwrap();
        assert_eq!(pdb.documents(), [r"C:\src\Program.cs"]);
        assert_eq!(pdb.find_document(r"c:/SRC/program.cs"), Some(1));
        // A client on another machine with a different root still binds by file name.
        assert_eq!(pdb.find_document("/home/me/src/Program.cs"), Some(1));
        assert_eq!(pdb.find_document("Other.cs"), None);

        let b = pdb.bind_line(1, 11).unwrap();
        assert_eq!(
            b,
            LineBinding {
                method_token: 0x0600_0002,
                il_offset: 2,
                line: 11
            }
        );
        // A line with no code inside a method binds to the next line that has some.
        assert_eq!(pdb.bind_line(1, 7), None);
        assert_eq!(pdb.bind_line(1, 6).unwrap().method_token, 0x0600_0001);

        let at = pdb.point_at(0x0600_0002, 9).unwrap();
        assert_eq!(at.start_line, 11);
        assert_eq!(pdb.point_at(0x0600_0002, 15).unwrap().start_line, 12);

        assert_eq!(
            pdb.locals_at(0x0600_0002, 7),
            vec![LocalVariable {
                index: 0,
                name: "counter".into()
            }]
        );
        assert!(pdb.locals_at(0x0600_0002, 25).is_empty());
        assert!(pdb.locals_at(0x0600_0001, 0).is_empty());
    }
}
