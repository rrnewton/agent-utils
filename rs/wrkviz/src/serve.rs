//! `wrkviz serve`: the loopback static server for a built
//! archive, at parity with the Python edition's
//! `standalone_server` (which is also copied into archives):
//! `.gz` companions as complete representations, strong
//! SHA-256 etags, byte ranges, and content-hashed immutable
//! caching. Loopback only, like the oracle.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

const REVALIDATE: &str = "public, no-cache";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const MAX_RANGE_BYTES: u64 = 16 << 20;
const MAX_INFLATE_BYTES: usize = 512 << 20;

static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `serve`'s pre-flight, then serve until killed.
pub fn serve(archive: &Path, port: u16, open_browser: bool) -> u8 {
    let root = crate::json::resolve_path(archive);
    if !root.is_dir() {
        eprintln!(
            "wrkviz: archive directory does not exist: {}",
            archive.display()
        );
        return 2;
    }
    if !root.join("index.html").is_file() {
        eprintln!(
            "wrkviz: archive has no index.html; run `wrkviz build`: {}",
            root.display()
        );
        return 2;
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 1;
        }
    };
    let actual_port = listener
        .local_addr()
        .map(|addr| addr.port())
        .unwrap_or(port);
    let url = format!("http://127.0.0.1:{actual_port}/");
    println!("wrkviz: serving {} at {url}", root.display());
    let _ = std::io::stdout().flush();
    if open_browser {
        let _ = std::process::Command::new("xdg-open")
            .arg(&url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    let _ = ctrlc::set_handler(|| {
        INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let _ = listener.set_nonblocking(true);
    loop {
        if INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst) {
            println!("\nwrkviz: stopped");
            return 0;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let root = root.clone();
                std::thread::spawn(move || handle(stream, &root));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

fn accepts_gzip(header: &str) -> bool {
    let mut gzip_quality: Option<f64> = None;
    let mut star_quality: Option<f64> = None;
    for raw_item in header.split(',') {
        let mut parts = raw_item.split(';');
        let coding = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if coding.is_empty() {
            continue;
        }
        let mut quality = 1.0f64;
        for parameter in parts {
            let Some((name, value)) = parameter.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("q") {
                quality = value.trim().parse::<f64>().unwrap_or(0.0);
                if !(0.0..=1.0).contains(&quality) {
                    quality = 0.0;
                }
            }
        }
        if coding == "gzip" {
            gzip_quality = Some(quality);
        } else if coding == "*" {
            star_quality = Some(quality);
        }
    }
    match gzip_quality {
        Some(quality) => quality > 0.0,
        None => star_quality.unwrap_or(0.0) > 0.0,
    }
}

fn cache_control_for_path(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_hashed = {
        let bytes = name.as_bytes();
        let stem_ok = bytes.len() >= 64
            && bytes[..64]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        stem_ok
            && (bytes.len() == 64
                || (bytes.len() > 65
                    && bytes[64] == b'.'
                    && bytes[65].is_ascii_alphanumeric()
                    && bytes[65..].iter().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })))
    };
    if is_hashed {
        IMMUTABLE
    } else {
        REVALIDATE
    }
}

fn strong_etag(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut hex = String::new();
    for byte in digest.iter() {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("\"sha256-{hex}\"")
}

fn etag_matches(raw_header: &str, etag: &str) -> bool {
    raw_header.split(',').any(|raw_value| {
        let value = raw_value.trim();
        value == "*" || value == etag || value.strip_prefix("W/") == Some(etag)
    })
}

fn parse_big(digits: &str) -> Option<i128> {
    if digits.len() > 30 {
        return Some(i128::MAX / 2);
    }
    digits.parse().ok()
}

fn parse_byte_range(header: &str, size: u64) -> Option<(i128, i128)> {
    let size = size as i128;
    let header = header.trim();
    let rest = header.strip_prefix("bytes=")?;
    let (raw_first, raw_last) = rest.split_once('-')?;
    if raw_last.contains('-') {
        return None;
    }
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if raw_first.is_empty() && raw_last.is_empty() {
        return None;
    }
    if raw_first.is_empty() {
        if !digits(raw_last) {
            return None;
        }
        let length: i128 = parse_big(raw_last)?;
        if length == 0 {
            return None;
        }
        return Some(((size - length).max(0), size - 1));
    }
    if !digits(raw_first) {
        return None;
    }
    let first: i128 = parse_big(raw_first)?;
    if first >= size {
        return Some((size, size - 1));
    }
    if raw_last.is_empty() {
        return Some((first, size - 1));
    }
    if !digits(raw_last) {
        return None;
    }
    let last: i128 = parse_big(raw_last)?;
    let last = last.min(size - 1);
    if last < first {
        return None;
    }
    Some((first, last))
}

fn content_type(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let extension = match lower.rfind('.') {
        Some(index) if index > 0 => &lower[index + 1..],
        _ => "",
    };
    match extension {
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "js" => "application/javascript",
        "mjs" => "text/javascript",
        "css" => "text/css",
        "gz" => "application/gzip",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "csv" => "text/csv",
        "tar" => "application/x-tar",
        "zip" => "application/zip",
        "log" => "text/plain",
        "tsv" => "text/tab-separated-values",
        "otf" => "font/otf",
        "xhtml" => "application/xhtml+xml",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/x-wav",
        "flac" => "audio/x-flac",
        "sh" => "application/x-sh",
        "py" => "text/x-python",
        "sql" => "application/sql",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "xml" => "text/xml",
        _ => "application/octet-stream",
    }
}

fn percent_decode(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn translate_path(root: &Path, raw_path: &str) -> PathBuf {
    let path_part = raw_path.split(['?', '#']).next().unwrap_or("");
    let decoded = String::from_utf8_lossy(&percent_decode(path_part)).into_owned();
    let mut components: Vec<String> = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other.to_string()),
        }
    }
    let mut out = root.to_path_buf();
    for component in components {
        out.push(component);
    }
    out
}

fn http_date(mtime: std::time::SystemTime) -> String {
    let seconds = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|moment| moment.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

fn now_http_date() -> String {
    http_date(std::time::SystemTime::now())
}

fn parse_http_date(value: &str) -> Option<i64> {
    let value = value.trim();
    for format in [
        "%a, %d %b %Y %H:%M:%S GMT",
        "%A, %d-%b-%y %H:%M:%S GMT",
        "%a %b %d %H:%M:%S %Y",
    ] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed.and_utc().timestamp());
        }
    }
    None
}

struct Response {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn error_response(status: u16, message: &str) -> Response {
    error_response_explain(status, message, None)
}

fn error_response_explain(status: u16, message: &str, explain: Option<&str>) -> Response {
    let explanation = explain.unwrap_or(match status {
        400 => "Bad request syntax or unsupported method.",
        404 => "Nothing matches the given URI.",
        414 => "URI is too long.",
        431 => {
            "The server is unwilling to process the request because its header fields are too large."
        }
        501 => "Server does not support this operation.",
        505 => "Cannot fulfill request.",
        _ => "",
    });
    let body = format!(
        "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {status}</p>\n        <p>Message: {}.</p>\n        <p>Error code explanation: {status} - {}</p>\n    </body>\n</html>\n",
        html_escape(message),
        html_escape(explanation),
    );
    let body = body.into_bytes();
    Response {
        status,
        reason: message.to_string(),
        headers: vec![
            ("Connection".to_string(), "close".to_string()),
            (
                "Content-Type".to_string(),
                "text/html;charset=utf-8".to_string(),
            ),
            ("Content-Length".to_string(), body.len().to_string()),
            ("Cache-Control".to_string(), REVALIDATE.to_string()),
            ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
            ("Referrer-Policy".to_string(), "no-referrer".to_string()),
        ],
        body,
    }
}

fn is_file_not_symlink(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.is_file() && !meta.file_type().is_symlink(),
        Err(_) => false,
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

fn py_repr(text: &str) -> String {
    let mut out = String::from("'");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('\'');
    out
}

fn read_line(reader: &mut impl std::io::BufRead, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut taken = reader.take((limit + 1) as u64);
    taken.read_until(b'\n', &mut out)?;
    Ok(out)
}

fn strip_line_end(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

fn handle(stream: TcpStream, root: &Path) {
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let mut reader = std::io::BufReader::new(stream);
    let raw_line = match read_line(&mut reader, 65537) {
        Ok(line) => line,
        Err(_) => return,
    };
    if raw_line.len() > 65536 {
        let response = error_response(414, "Request-URI Too Long");
        log_error("", "", 414, "Request-URI Too Long");
        write_response(&mut writer, &response, false, false);
        log_request("", "", "", 414);
        return;
    }
    if raw_line.is_empty() {
        return;
    }
    let request_line = latin1(strip_line_end(&raw_line));
    let words: Vec<&str> = request_line.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let mut http09 = true;
    if words.len() >= 3 {
        let version = words[words.len() - 1];
        let parsed = (|| -> Option<(u32, u32)> {
            let rest = version.strip_prefix("HTTP/")?;
            let (major, minor) = rest.split_once('.')?;
            if major.len() > 10 || minor.len() > 10 {
                return None;
            }
            if !major.bytes().all(|byte| byte.is_ascii_digit())
                || !minor.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            Some((major.parse().ok()?, minor.parse().ok()?))
        })();
        match parsed {
            None => {
                let message = format!("Bad request version ({})", py_repr(version));
                let response = error_response(400, &message);
                log_error("", "", 400, &message);
                write_response(&mut writer, &response, false, true);
                log_request(&request_line, "", "", 400);
                return;
            }
            Some((major, _)) if major >= 2 => {
                let base = version.strip_prefix("HTTP/").unwrap_or(version);
                let message = format!("Invalid HTTP version ({base})");
                let response = error_response(505, &message);
                log_error("", "", 505, &message);
                write_response(&mut writer, &response, false, true);
                log_request(&request_line, "", "", 505);
                return;
            }
            Some((0, 9)) => {}
            Some(_) => http09 = false,
        }
    }
    if !(2..=3).contains(&words.len()) {
        let message = format!("Bad request syntax ({})", py_repr(&request_line));
        let response = error_response(400, &message);
        log_error("", "", 400, &message);
        write_response(&mut writer, &response, false, http09);
        log_request(&request_line, "", "", 400);
        return;
    }
    let method = words[0].to_string();
    let raw_path = words[1].to_string();
    if words.len() == 2 {
        if method != "GET" {
            let message = format!("Bad HTTP/0.9 request type ({})", py_repr(&method));
            let response = error_response(400, &message);
            log_error("", "", 400, &message);
            write_response(&mut writer, &response, false, true);
            log_request(&request_line, "", "", 400);
            return;
        }
        http09 = true;
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut header_lines = 0usize;
    loop {
        let line = match read_line(&mut reader, 65537) {
            Ok(line) => line,
            Err(_) => return,
        };
        if line.is_empty() {
            break;
        }
        header_lines += 1;
        if line.len() > 65536 {
            let response = error_response_explain(431, "Line too long", Some("header line"));
            log_error("", &raw_path, 431, "Line too long");
            write_response(&mut writer, &response, false, http09);
            log_request(&request_line, "", &raw_path, 431);
            return;
        }
        if header_lines > 100 {
            let response =
                error_response_explain(431, "Too many headers", Some("got more than 100 headers"));
            log_error("", &raw_path, 431, "Too many headers");
            write_response(&mut writer, &response, false, http09);
            log_request(&request_line, "", &raw_path, 431);
            return;
        }
        let text = latin1(strip_line_end(&line));
        if text.is_empty() {
            break;
        }
        if let Some((name, value)) = text.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let header = |name: &str| -> String {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if method != "GET" && method != "HEAD" {
        let message = format!("Unsupported method ({})", py_repr(&method));
        let response = error_response(501, &message);
        log_error(&method, &raw_path, 501, &message);
        write_response(&mut writer, &response, false, http09);
        log_request(&request_line, &method, &raw_path, 501);
        return;
    }
    let response = respond(root, &raw_path, &header, &method);
    if response.status == 404 {
        log_error(&method, &raw_path, 404, "File not found");
    }
    write_response(&mut writer, &response, method == "HEAD", http09);
    log_request(&request_line, &method, &raw_path, response.status);
}

fn log_suppressed(method: &str, raw_path: &str) -> bool {
    method == "GET" && (raw_path == "/" || raw_path == "/index.html")
}

fn log_stamp() -> String {
    chrono::Local::now().format("%d/%b/%Y %H:%M:%S").to_string()
}

fn log_error(method: &str, raw_path: &str, code: u16, message: &str) {
    if log_suppressed(method, raw_path) {
        return;
    }
    eprintln!(
        "127.0.0.1 - - [{}] code {code}, message {message}",
        log_stamp()
    );
}

fn log_request(request_line: &str, method: &str, raw_path: &str, code: u16) {
    if log_suppressed(method, raw_path) {
        return;
    }
    eprintln!(
        "127.0.0.1 - - [{}] \"{request_line}\" {code} -",
        log_stamp()
    );
}

fn write_response(stream: &mut TcpStream, response: &Response, head_only: bool, http09: bool) {
    if http09 {
        if !head_only {
            let _ = stream.write_all(&response.body);
            let _ = stream.flush();
        }
        return;
    }
    let mut out = Vec::new();
    out.extend_from_slice(
        format!("HTTP/1.0 {} {}\r\n", response.status, response.reason).as_bytes(),
    );
    out.extend_from_slice(b"Server: SimpleHTTP/0.6\r\n");
    out.extend_from_slice(format!("Date: {}\r\n", now_http_date()).as_bytes());
    for (name, value) in &response.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    if response.status != 304
        && !response
            .headers
            .iter()
            .any(|(name, _)| name == "Content-Length")
    {
        out.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    if !head_only {
        out.extend_from_slice(&response.body);
    }
    let _ = stream.write_all(&out);
    let _ = stream.flush();
}

fn respond(
    root: &Path,
    raw_path: &str,
    header: &dyn Fn(&str) -> String,
    _method: &str,
) -> Response {
    let translated = translate_path(root, raw_path);
    if translated
        .components()
        .any(|part| part.as_os_str() == "source_snapshots")
    {
        return error_response(404, "File not found");
    }
    // Directory handling.
    if translated.is_dir() {
        let request_path = raw_path.split(['?', '#']).next().unwrap_or("");
        if !request_path.ends_with('/') {
            let location = {
                let (before_fragment, fragment) = match raw_path.split_once('#') {
                    Some((head, tail)) => (head, Some(tail)),
                    None => (raw_path, None),
                };
                let (path_part, query) = match before_fragment.split_once('?') {
                    Some((head, tail)) => (head, Some(tail)),
                    None => (before_fragment, None),
                };
                let mut location = format!("{path_part}/");
                if let Some(query) = query {
                    location.push('?');
                    location.push_str(query);
                }
                if let Some(fragment) = fragment {
                    location.push('#');
                    location.push_str(fragment);
                }
                location
            };
            return Response {
                status: 301,
                reason: "Moved Permanently".to_string(),
                headers: vec![
                    ("Location".to_string(), location),
                    ("Content-Length".to_string(), "0".to_string()),
                    ("Cache-Control".to_string(), REVALIDATE.to_string()),
                    ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                    ("Referrer-Policy".to_string(), "no-referrer".to_string()),
                ],
                body: Vec::new(),
            };
        }
        for index_name in ["index.html", "index.htm"] {
            let candidate = translated.join(index_name);
            if candidate.is_file() {
                return respond_file(root, &candidate, &candidate, header);
            }
        }
        return error_response(404, "File not found");
    }
    let source_path = translated.clone();
    let sidecar_path = {
        let mut name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        name.push_str(".gz");
        source_path.with_file_name(name)
    };
    respond_file(root, &source_path, &sidecar_path, header)
}

fn respond_file(
    _root: &Path,
    source_path: &Path,
    sidecar_or_self: &Path,
    header: &dyn Fn(&str) -> String,
) -> Response {
    // When invoked from the directory branch, the file is its
    // own source and the sidecar is derived here; when invoked
    // from respond, the arguments are source and sidecar.
    let source_path = source_path.to_path_buf();
    let sidecar_path = {
        let mut name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if sidecar_or_self == source_path {
            name.push_str(".gz");
            source_path.with_file_name(name)
        } else {
            sidecar_or_self.to_path_buf()
        }
    };
    let source_ok = is_file_not_symlink(&source_path);
    let sidecar_ok = is_file_not_symlink(&sidecar_path);
    let source_is_symlink = std::fs::symlink_metadata(&source_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if source_is_symlink && source_path.is_file() && !sidecar_ok {
        let mtime = std::fs::metadata(&source_path)
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let ims = header("if-modified-since");
        if header("if-none-match").is_empty() && !ims.is_empty() {
            if let Some(ims_seconds) = parse_http_date(&ims) {
                let mtime_seconds = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or(0);
                if mtime_seconds <= ims_seconds {
                    return Response {
                        status: 304,
                        reason: "Not Modified".to_string(),
                        headers: vec![
                            ("Cache-Control".to_string(), REVALIDATE.to_string()),
                            ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                            ("Referrer-Policy".to_string(), "no-referrer".to_string()),
                        ],
                        body: Vec::new(),
                    };
                }
            }
        }
        let bytes = match std::fs::read(&source_path) {
            Ok(bytes) => bytes,
            Err(_) => return error_response(404, "File not found"),
        };
        return Response {
            status: 200,
            reason: "OK".to_string(),
            headers: vec![
                (
                    "Content-type".to_string(),
                    content_type(&source_path).to_string(),
                ),
                ("Content-Length".to_string(), bytes.len().to_string()),
                ("Last-Modified".to_string(), http_date(mtime)),
                ("Cache-Control".to_string(), REVALIDATE.to_string()),
                ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                ("Referrer-Policy".to_string(), "no-referrer".to_string()),
            ],
            body: bytes,
        };
    }
    let gz_only = sidecar_ok && !source_ok;
    let vary = sidecar_ok;
    let raw_range = header("range");
    let use_gzip = sidecar_ok && raw_range.is_empty() && accepts_gzip(&header("accept-encoding"));
    let mut inflated: Option<Vec<u8>> = None;
    if gz_only && !use_gzip {
        match read_inflated(&sidecar_path) {
            Some(bytes) => inflated = Some(bytes),
            None => return error_response(404, "File not found"),
        }
    }
    let selected_path = if inflated.is_some() || use_gzip {
        sidecar_path.clone()
    } else {
        source_path.clone()
    };
    let selected_bytes: Vec<u8> = if let Some(bytes) = &inflated {
        bytes.clone()
    } else {
        match std::fs::read(&selected_path) {
            Ok(bytes) => bytes,
            Err(_) => return error_response(404, "File not found"),
        }
    };
    let source_stat_path = if gz_only {
        sidecar_path.clone()
    } else {
        source_path.clone()
    };
    let source_mtime = std::fs::metadata(&source_stat_path)
        .and_then(|meta| meta.modified())
        .unwrap_or(std::time::UNIX_EPOCH);
    let etag = if inflated.is_some() {
        let base = strong_etag(&std::fs::read(&sidecar_path).unwrap_or_default());
        format!("{}-identity\"", &base[..base.len() - 1])
    } else {
        strong_etag(&selected_bytes)
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    if etag_matches(&header("if-none-match"), &etag) {
        headers.push(("ETag".to_string(), etag));
        headers.push(("Last-Modified".to_string(), http_date(source_mtime)));
        headers.push((
            "Cache-Control".to_string(),
            cache_control_for_path(&source_path).to_string(),
        ));
        if vary {
            headers.push(("Vary".to_string(), "Accept-Encoding".to_string()));
        }
        if use_gzip {
            headers.push(("Content-Encoding".to_string(), "gzip".to_string()));
        }
        headers.push(("X-Content-Type-Options".to_string(), "nosniff".to_string()));
        headers.push(("Referrer-Policy".to_string(), "no-referrer".to_string()));
        return Response {
            status: 304,
            reason: "Not Modified".to_string(),
            headers,
            body: Vec::new(),
        };
    }
    let size = selected_bytes.len() as u64;
    let if_range = header("if-range");
    let mut selection =
        if !raw_range.is_empty() && (if_range.is_empty() || etag_matches(&if_range, &etag)) {
            parse_byte_range(&raw_range, size)
        } else {
            None
        };
    if let Some((first, last)) = selection {
        if last >= first && last - first + 1 > MAX_RANGE_BYTES as i128 {
            selection = None;
        }
    }
    if let Some((first, last)) = selection {
        if last < first {
            headers.push(("Content-Range".to_string(), format!("bytes */{size}")));
            headers.push(("Content-Length".to_string(), "0".to_string()));
            headers.push(("ETag".to_string(), etag));
            headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
            headers.push((
                "Cache-Control".to_string(),
                cache_control_for_path(&source_path).to_string(),
            ));
            headers.push(("X-Content-Type-Options".to_string(), "nosniff".to_string()));
            headers.push(("Referrer-Policy".to_string(), "no-referrer".to_string()));
            return Response {
                status: 416,
                reason: "Requested Range Not Satisfiable".to_string(),
                headers,
                body: Vec::new(),
            };
        }
    }
    let (status, body, content_range) = match selection {
        Some((first, last)) => (
            206,
            selected_bytes[first as usize..=last as usize].to_vec(),
            Some(format!("bytes {first}-{last}/{size}")),
        ),
        None => (200, selected_bytes, None),
    };
    headers.push((
        "Content-Type".to_string(),
        content_type(&source_path).to_string(),
    ));
    if let Some(range) = content_range {
        headers.push(("Content-Range".to_string(), range));
    }
    headers.push(("Content-Length".to_string(), body.len().to_string()));
    headers.push(("Last-Modified".to_string(), http_date(source_mtime)));
    headers.push(("ETag".to_string(), etag));
    headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
    headers.push((
        "Cache-Control".to_string(),
        cache_control_for_path(&source_path).to_string(),
    ));
    if vary {
        headers.push(("Vary".to_string(), "Accept-Encoding".to_string()));
    }
    if use_gzip {
        headers.push(("Content-Encoding".to_string(), "gzip".to_string()));
    }
    headers.push(("X-Content-Type-Options".to_string(), "nosniff".to_string()));
    headers.push(("Referrer-Policy".to_string(), "no-referrer".to_string()));
    Response {
        status,
        reason: if status == 206 {
            "Partial Content".to_string()
        } else {
            "OK".to_string()
        },
        headers,
        body,
    }
}

fn read_inflated(path: &Path) -> Option<Vec<u8>> {
    let compressed = std::fs::read(path).ok()?;
    let mut decoded = Vec::new();
    std::io::Read::by_ref(&mut flate2::read::MultiGzDecoder::new(&compressed[..]))
        .take((MAX_INFLATE_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .ok()?;
    if decoded.len() > MAX_INFLATE_BYTES {
        return None;
    }
    Some(decoded)
}
