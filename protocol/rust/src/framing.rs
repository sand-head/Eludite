//! `Content-Length` framing used by LSP and DAP over stdio and TCP.

use std::io::{self, BufRead, Write};

/// Write one framed message.
pub fn write_message(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

/// Read one framed message. Returns `Ok(None)` on clean EOF before any header.
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut saw_header = false;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return if saw_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF in headers",
                ))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length =
                Some(value.trim().parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
                })?);
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_two_messages() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"a":1}"#).unwrap();
        write_message(&mut buf, "{\"b\":\"é\"}".as_bytes()).unwrap();
        let mut r = io::Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), br#"{"a":1}"#);
        assert_eq!(
            read_message(&mut r).unwrap().unwrap(),
            "{\"b\":\"é\"}".as_bytes()
        );
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn extra_headers_and_missing_length() {
        let mut r = io::Cursor::new(
            b"Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\n{}".to_vec(),
        );
        assert_eq!(read_message(&mut r).unwrap().unwrap(), b"{}");
        let mut r = io::Cursor::new(b"X: y\r\n\r\n".to_vec());
        assert!(read_message(&mut r).is_err());
    }
}
