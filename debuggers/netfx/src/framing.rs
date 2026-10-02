//! `Content-Length` framing for DAP messages (the DAP specification's base protocol).

use std::io::{self, BufRead, Write};

/// The largest message body the adapter accepts (a guard against a garbage header).
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// Reads one framed message body. `Ok(None)` is a clean end of stream before any header byte.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            if first {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended inside a message header",
            ));
        }
        first = false;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(invalid(format!("malformed header line {trimmed:?}")));
        };
        if name.trim().eq_ignore_ascii_case("Content-Length") {
            let len: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid(format!("bad Content-Length {:?}", value.trim())))?;
            if len > MAX_BODY {
                return Err(invalid(format!("Content-Length {len} exceeds {MAX_BODY}")));
            }
            content_length = Some(len);
        }
        // Other headers (Content-Type) are allowed and ignored.
    }
    let len =
        content_length.ok_or_else(|| invalid("message header has no Content-Length".into()))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Writes one framed message body and flushes.
pub fn write_message(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn round_trips_two_messages() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"seq":1}"#).unwrap();
        write_message(&mut buf, "{\"x\":\"\u{e9}\"}".as_bytes()).unwrap();
        let mut reader = BufReader::new(Cursor::new(buf));
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), br#"{"seq":1}"#);
        assert_eq!(
            read_message(&mut reader).unwrap().unwrap(),
            "{\"x\":\"\u{e9}\"}".as_bytes()
        );
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn content_length_counts_bytes_not_chars() {
        let mut buf = Vec::new();
        write_message(&mut buf, "\u{e9}".as_bytes()).unwrap();
        assert!(buf.starts_with(b"Content-Length: 2\r\n\r\n"));
    }

    #[test]
    fn accepts_extra_headers_and_any_case() {
        let raw = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        let mut reader = BufReader::new(Cursor::new(&raw[..]));
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn rejects_missing_length_and_truncation() {
        let mut reader = BufReader::new(Cursor::new(&b"X-Other: 1\r\n\r\n{}"[..]));
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut reader = BufReader::new(Cursor::new(&b"Content-Length: 10\r\n\r\n{}"[..]));
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut reader = BufReader::new(Cursor::new(&b"Content-Length: 2\r\n"[..]));
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut reader = BufReader::new(Cursor::new(&b"Content-Length: nope\r\n\r\n"[..]));
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
