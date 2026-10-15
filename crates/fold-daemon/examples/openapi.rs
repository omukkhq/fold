use std::io::{self, Write};

use fold_daemon::http;

/// Prints the OpenAPI document of the HTTP API for `make gen-api` and `make check-api`. It starts no daemon and reads no project, so it runs on a clean checkout, and it ships in no binary.
fn main() -> io::Result<()> {
    let text = http::openapi::to_json(&http::openapi())?;
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")
}
