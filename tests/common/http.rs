use std::io::BufRead;

/// The body, however the server chose to frame it.
///
/// `tiny_http` answers with `Content-Length` while it can and switches to
/// `Transfer-Encoding: chunked` once a response outgrows its buffer. Every
/// page was small enough for the first branch until the map arrived at
/// rather more than a megabyte, and a client that only knew lengths read
/// every one of those pages as empty -- with a `200` in hand, which is the
/// worst way to be wrong. The client is ours, so it learns the other
/// framing rather than the pages staying small enough to avoid it.
pub(super) fn read_body(
    reader: &mut impl BufRead,
    content_length: usize,
    chunked: bool,
) -> Vec<u8> {
    if !chunked {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).unwrap();
        return body;
    }
    let mut body = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        // A chunk size is hex, and may carry extensions after a semicolon
        // that nothing here needs.
        let size = usize::from_str_radix(
            header
                .trim_end_matches(['\r', '\n'])
                .split(';')
                .next()
                .unwrap_or("")
                .trim(),
            16,
        )
        .unwrap_or_else(|_| panic!("not a chunk size: {header:?}"));
        if size == 0 {
            break;
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk).unwrap();
        body.extend_from_slice(&chunk);
        // The CRLF that closes the chunk.
        let mut end = [0u8; 2];
        reader.read_exact(&mut end).unwrap();
    }
    body
}
