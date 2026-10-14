use std::io::{self, Write};

use fold_daemon::http;

use crate::output;

/// Prints the OpenAPI document. It starts no daemon and reads no project, so it runs on a clean checkout.
pub fn run(out: &mut impl Write) -> io::Result<()> {
    let text = http::openapi::to_json(&http::openapi())?;
    output::line(out, &text)
}
