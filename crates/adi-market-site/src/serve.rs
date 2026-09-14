//! A preview server: the generated site, over HTTP, from memory.
//!
//! It exists so the site can be *looked at* before it is published anywhere — the output is
//! static files, and `file://` does not answer the two questions that matter (does a directory URL
//! resolve to its `index.html`, and do the relative stylesheet and font paths hold two levels
//! down). It is a preview and says so: single-threaded per connection, no caching headers, no
//! compression, no TLS, and it binds loopback.
//!
//! The site is rendered once, at start. A change to a manifest — or to this crate's own CSS, which
//! is compiled in — needs the command run again.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use crate::File;

/// Serve `files` on `addr` until the process is stopped.
///
/// # Errors
/// If the port cannot be bound.
pub fn run(addr: SocketAddr, files: Vec<File>) -> std::io::Result<()> {
    let site: HashMap<String, File> = files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();
    let listener = TcpListener::bind(addr)?;
    println!("the marketplace is at http://{addr}/  ({} files)", site.len());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        if let Err(e) = answer(&mut stream, &site) {
            // A reader who closed the tab mid-response is not an event; the preview keeps serving.
            eprintln!("preview: {e}");
        }
    }
    Ok(())
}

/// One request.
fn answer(stream: &mut TcpStream, site: &HashMap<String, File>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // Drain the headers so the client sees a clean close rather than a reset.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let key = path.trim_start_matches('/');

    // A directory without its trailing slash resolves relative links one level too high, and the
    // canonical URL of every item page has the slash — so say so rather than serving both.
    if !key.is_empty() && !key.ends_with('/') && site.contains_key(&format!("{key}/index.html")) {
        return write_head(stream, 301, "Moved Permanently", &[("Location", &format!("/{key}/"))]);
    }
    let key = if key.is_empty() || key.ends_with('/') {
        format!("{key}index.html")
    } else {
        key.to_string()
    };
    match site.get(&key) {
        Some(file) => write_file(stream, file),
        None => write_missing(stream),
    }
}

/// The file, with the one header that decides whether a browser renders or downloads it.
fn write_file(stream: &mut TcpStream, file: &File) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type(&file.path),
        file.bytes.len(),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&file.bytes)?;
    stream.flush()
}

/// A path the generated site does not have. Plain text on purpose: the published site's 404 is
/// whatever its host serves, and a preview that drew a designed one would be previewing a page
/// that does not exist.
fn write_missing(stream: &mut TcpStream) -> std::io::Result<()> {
    let body = "not in the generated site\n";
    let head = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// A response with headers and no body.
fn write_head(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    headers: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n");
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

/// What a file is, by its extension. The site has six kinds of file in it and nothing else.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("xml") => "application/xml; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_file_the_generator_writes_has_a_type() {
        let site = crate::render(&crate::tests::fixture_site(), &[crate::tests::fixture_source()]);
        for file in site {
            assert_ne!(
                content_type(&file.path),
                "application/octet-stream",
                "{} would download rather than render",
                file.path
            );
        }
    }
}
