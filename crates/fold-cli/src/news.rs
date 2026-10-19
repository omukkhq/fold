use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::output;
use crate::serve::{Dirs, ServeError};

/// The public endpoint that stores an address for release news.
pub const DEFAULT_URL: &str = "https://omukk.dev/fold/subscribe";
pub const URL_ENV: &str = "FOLD_NEWS_URL";
pub const NO_PROMPT_ENV: &str = "FOLD_NO_NEWS_PROMPT";
const TIMEOUT: Duration = Duration::from_secs(5);
const QUESTION: &str = "Get fold release news by email? fld sends the address you type and its own version, once, to Omukk. No usage data.";
const ASK: &str = "Email (press Enter to skip): ";

#[derive(Args, Debug)]
pub struct SubscribeArgs {
    /// The address to send release news to; asked for on a terminal when left out
    email: Option<String>,
}

#[derive(Debug, Error)]
pub enum NewsError {
    #[error("the given address is not an email address")]
    InvalidAddress,
    #[error("no address given and fld cannot ask: it does not run on a terminal")]
    NotInteractive,
    #[error("cannot reach Omukk: {0}")]
    Send(String),
    #[error("{URL_ENV} must be an https address, or an http address on this machine")]
    Endpoint,
    #[error(transparent)]
    Config(#[from] ServeError),
    #[error("cannot record the answer in {path}: {source}")]
    Record { path: PathBuf, source: io::Error },
    #[error("cannot write the answer: {0}")]
    Write(#[from] io::Error),
}

impl NewsError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Config(error) => error.code(),
            _ => None,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::InvalidAddress => Some("give an address such as ana@example.com"),
            Self::NotInteractive => Some("run `fld subscribe ana@example.com`"),
            Self::Send(_) => Some("check the network and run `fld subscribe` again"),
            Self::Endpoint => Some("unset FOLD_NEWS_URL, or point it at https"),
            Self::Config(_) | Self::Record { .. } | Self::Write(_) => None,
        }
    }
}

/// What the user answered, kept in `news.toml` of the fold config folder. The address itself is never stored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub asked: bool,
    pub subscribed: bool,
}

/// The environment of the news question, read once.
#[derive(Debug, Clone)]
pub struct Settings {
    pub url: Option<String>,
    pub no_prompt: bool,
    pub timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            url: None,
            no_prompt: false,
            timeout: TIMEOUT,
        }
    }
}

impl Settings {
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// A CI run counts as an opt-out, because nobody there can answer the question.
    fn from_vars(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        Self {
            url: var(URL_ENV)
                .and_then(|url| url.into_string().ok())
                .filter(|url| !url.is_empty()),
            no_prompt: set(NO_PROMPT_ENV) || set("CI"),
            timeout: TIMEOUT,
        }
    }

    /// The endpoint to post to. A plain http address is allowed only for this machine, so that no address leaves it unencrypted.
    fn endpoint(&self) -> Result<&str, NewsError> {
        let Some(url) = self.url.as_deref() else {
            return Ok(DEFAULT_URL);
        };
        if url.starts_with("https://") {
            return Ok(url);
        }
        let host = url
            .strip_prefix("http://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .map(|authority| authority.rsplit('@').next().unwrap_or(authority));
        let loopback = host.is_some_and(|host| {
            ["127.0.0.1", "localhost", "[::1]"].iter().any(|name| {
                host == *name
                    || host.strip_prefix(name).is_some_and(|port| {
                        port.starts_with(':') && port[1..].parse::<u16>().is_ok()
                    })
            })
        });
        if loopback {
            Ok(url)
        } else {
            Err(NewsError::Endpoint)
        }
    }
}

/// A basic shape check: one `@`, text on both sides, a dot inside the domain, no spaces or control characters.
pub fn valid_address(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    address.len() <= 254
        && !address.contains(|c: char| c.is_whitespace() || c.is_control())
        && !domain.contains('@')
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn answer_file(dirs: &Dirs) -> Result<PathBuf, ServeError> {
    Ok(dirs.config_base()?.join("fold").join("news.toml"))
}

fn read_answer(path: &std::path::Path) -> Answer {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn record(path: &std::path::Path, answer: Answer) -> Result<(), NewsError> {
    let fail = |source| NewsError::Record {
        path: path.to_owned(),
        source,
    };
    let text = toml::to_string(&answer).map_err(|error| fail(io::Error::other(error)))?;
    fold_vcs::atomic::write_atomic(path, text.as_bytes()).map_err(fail)
}

/// Posts the address and nothing else. Any 2xx is a success.
fn send(url: &str, address: &str, timeout: Duration) -> Result<(), NewsError> {
    let body = serde_json::json!({
        "email": address,
        "source": "fld",
        "version": env!("CARGO_PKG_VERSION"),
    })
    .to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body)
        .map(|_| ())
        .map_err(|error| NewsError::Send(error.to_string()))
}

/// Asks for an address, twice at most. `None` when the user skips or gives no valid address.
fn read_address(input: &mut impl BufRead, out: &mut impl Write) -> io::Result<Option<String>> {
    for attempt in 0..2 {
        write!(out, "{ASK}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        let address = line.trim();
        if address.is_empty() {
            return Ok(None);
        }
        if valid_address(address) {
            return Ok(Some(address.to_owned()));
        }
        if attempt == 0 {
            output::line(out, "That is not an email address.")?;
        }
    }
    Ok(None)
}

/// The one-time question of the first interactive start. It never stops a start: a failed send prints one line and the answer is remembered as not subscribed.
pub fn ask_once(
    dirs: &Dirs,
    settings: &Settings,
    interactive: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) {
    if !interactive || settings.no_prompt {
        return;
    }
    let Ok(path) = answer_file(dirs) else {
        return;
    };
    if read_answer(&path).asked {
        return;
    }
    let Ok(url) = settings.endpoint() else {
        return;
    };
    if output::line(out, QUESTION).is_err() {
        return;
    }
    let Ok(address) = read_address(input, out) else {
        return;
    };
    let subscribed = match address {
        Some(address) => match send(url, &address, settings.timeout) {
            Ok(()) => {
                let _ = output::line(out, "Subscribed. Thank you.");
                true
            }
            Err(_) => {
                let _ = output::line(out, "Could not reach Omukk; run fld subscribe later.");
                false
            }
        },
        None => false,
    };
    let _ = record(
        &path,
        Answer {
            asked: true,
            subscribed,
        },
    );
}

/// [`ask_once`] on the real terminal; it asks only when stdin and stdout are both terminals.
pub fn ask_once_on_terminal(dirs: &Dirs) {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    ask_once(
        dirs,
        &Settings::from_env(),
        interactive,
        &mut io::stdin().lock(),
        &mut io::stdout(),
    );
}

/// `fld subscribe [EMAIL]`.
pub fn subscribe(
    email: Option<&str>,
    dirs: &Dirs,
    settings: &Settings,
    interactive: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), NewsError> {
    let url = settings.endpoint()?;
    let address = match email {
        Some(address) if valid_address(address) => address.to_owned(),
        Some(_) => return Err(NewsError::InvalidAddress),
        None if !interactive => return Err(NewsError::NotInteractive),
        None => {
            output::line(out, QUESTION)?;
            match read_address(input, out)? {
                Some(address) => address,
                None => {
                    output::line(out, "Skipped; no address was sent.")?;
                    return Ok(());
                }
            }
        }
    };
    send(url, &address, settings.timeout)?;
    output::line(out, "Subscribed. Thank you.")?;
    record(
        &answer_file(dirs)?,
        Answer {
            asked: true,
            subscribed: true,
        },
    )
}

pub fn run(args: &SubscribeArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), NewsError> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    subscribe(
        args.email.as_deref(),
        dirs,
        &Settings::from_env(),
        interactive,
        &mut io::stdin().lock(),
        out,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;

    /// A one-request local server that answers `status` and reports the request it got.
    fn server(status: u16) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fold/subscribe", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut data = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let n = stream.read(&mut buffer).unwrap_or(0);
                    data.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&data).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        if n == 0 {
                            break;
                        }
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length || n == 0 {
                        break;
                    }
                }
                let _ = sender.send(String::from_utf8_lossy(&data).into_owned());
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        });
        (url, receiver)
    }

    fn dirs(root: &std::path::Path) -> Dirs {
        Dirs {
            home: Some(root.to_owned()),
            config_home: Some(root.join("config")),
            data_home: None,
            runtime_dir: None,
            library_runtime: false,
        }
    }

    fn settings(url: &str) -> Settings {
        Settings {
            url: Some(url.to_owned()),
            ..Settings::default()
        }
    }

    fn stored(dirs: &Dirs) -> Option<Answer> {
        let text = std::fs::read_to_string(answer_file(dirs).unwrap()).ok()?;
        Some(toml::from_str(&text).unwrap())
    }

    fn ask(dirs: &Dirs, settings: &Settings, interactive: bool, typed: &str) -> String {
        let mut out = Vec::new();
        ask_once(dirs, settings, interactive, &mut typed.as_bytes(), &mut out);
        String::from_utf8(out).unwrap()
    }

    fn received(receiver: &mpsc::Receiver<String>) -> String {
        receiver.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn the_first_interactive_start_asks_once() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        let shown = ask(&dirs, &settings(&url), true, "ana@example.com\n");
        assert!(shown.contains("Get fold release news by email?"));
        assert!(shown.contains("Email (press Enter to skip): "));
        received(&requests);
        let again = ask(&dirs, &settings(&url), true, "ana@example.com\n");
        assert_eq!(again, "");
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn enter_skips_and_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        ask(&dirs, &settings(&url), true, "\n");
        assert_eq!(
            stored(&dirs),
            Some(Answer {
                asked: true,
                subscribed: false
            })
        );
        assert_eq!(ask(&dirs, &settings(&url), true, "ana@example.com\n"), "");
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn an_address_is_sent_with_exactly_three_fields() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        ask(&dirs, &settings(&url), true, "ana@example.com\n");
        let request = received(&requests);
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /fold/subscribe "));
        assert!(lower.contains("content-type: application/json"));
        assert_eq!(lower.matches("content-type:").count(), 1);
        let body: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "email": "ana@example.com",
                "source": "fld",
                "version": env!("CARGO_PKG_VERSION"),
            })
        );
        assert_eq!(
            stored(&dirs),
            Some(Answer {
                asked: true,
                subscribed: true
            })
        );
        let file = std::fs::read_to_string(answer_file(&dirs).unwrap()).unwrap();
        assert!(!file.contains("ana@example.com"));
    }

    #[test]
    fn a_failing_endpoint_does_not_stop_the_start_and_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, _requests) = server(500);
        let shown = ask(&dirs, &settings(&url), true, "ana@example.com\n");
        assert!(shown.contains("Could not reach Omukk; run fld subscribe later.\n"));
        assert_eq!(
            stored(&dirs),
            Some(Answer {
                asked: true,
                subscribed: false
            })
        );
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}/", closed.local_addr().unwrap());
        drop(closed);
        let other = tempfile::tempdir().unwrap();
        let shown = ask(
            &self::dirs(other.path()),
            &settings(&dead),
            true,
            "ana@example.com\n",
        );
        assert!(shown.contains("Could not reach Omukk"));
    }

    #[test]
    fn an_invalid_address_is_asked_for_once_more_then_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        let shown = ask(
            &dirs,
            &settings(&url),
            true,
            "nope\nstill nope\nana@example.com\n",
        );
        assert_eq!(shown.matches("Email (press Enter to skip): ").count(), 2);
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(stored(&dirs).map(|a| a.subscribed), Some(false));
        let dir = tempfile::tempdir().unwrap();
        let dirs = self::dirs(dir.path());
        ask(&dirs, &settings(&url), true, "nope\nana@example.com\n");
        received(&requests);
    }

    #[test]
    fn a_start_without_terminals_or_with_the_opt_out_never_asks() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        assert_eq!(ask(&dirs, &settings(&url), false, "ana@example.com\n"), "");
        let mut quiet = settings(&url);
        quiet.no_prompt = true;
        assert_eq!(ask(&dirs, &quiet, true, "ana@example.com\n"), "");
        assert_eq!(stored(&dirs), None);
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn subscribe_sends_and_records() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(204);
        let mut out = Vec::new();
        subscribe(
            Some("ana@example.com"),
            &dirs,
            &settings(&url),
            false,
            &mut io::empty(),
            &mut out,
        )
        .unwrap();
        assert!(received(&requests).contains("ana@example.com"));
        assert_eq!(stored(&dirs).map(|a| a.subscribed), Some(true));
        assert!(String::from_utf8(out).unwrap().contains("Subscribed"));
    }

    #[test]
    fn subscribe_asks_on_a_terminal_and_refuses_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let (url, requests) = server(200);
        let mut out = Vec::new();
        let error = subscribe(
            None,
            &dirs,
            &settings(&url),
            false,
            &mut io::empty(),
            &mut out,
        )
        .unwrap_err();
        assert!(matches!(error, NewsError::NotInteractive));
        assert!(error.hint().is_some());
        let error = subscribe(
            Some("bad"),
            &dirs,
            &settings(&url),
            true,
            &mut io::empty(),
            &mut out,
        )
        .unwrap_err();
        assert!(matches!(error, NewsError::InvalidAddress));
        assert!(!error.to_string().contains("bad"));
        subscribe(
            None,
            &dirs,
            &settings(&url),
            true,
            &mut "ana@example.com\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        received(&requests);
        subscribe(
            None,
            &dirs,
            &settings(&url),
            true,
            &mut "\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        let (failing, _requests) = server(500);
        let error = subscribe(
            Some("ana@example.com"),
            &dirs,
            &settings(&failing),
            true,
            &mut io::empty(),
            &mut out,
        )
        .unwrap_err();
        assert!(matches!(error, NewsError::Send(_)));
    }

    #[test]
    fn plain_http_is_allowed_only_for_this_machine() {
        let url = |text: &str| settings(text).endpoint().map(str::to_owned);
        assert_eq!(
            url("https://example.com/x").unwrap(),
            "https://example.com/x"
        );
        assert!(url("http://127.0.0.1:8080/x").is_ok());
        assert!(url("http://localhost/x").is_ok());
        assert!(url("http://[::1]:1/x").is_ok());
        assert!(url("http://example.com/x").is_err());
        assert!(url("http://127.0.0.1.example.com/x").is_err());
        assert!(url("http://localhost@example.com/x").is_err());
        assert!(url("ftp://127.0.0.1/x").is_err());
        assert_eq!(Settings::default().endpoint().unwrap(), DEFAULT_URL);
    }

    #[test]
    fn a_server_that_never_answers_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (held, kept) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let _ = held.send(stream);
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let slow = Settings {
            timeout: Duration::from_millis(200),
            ..settings(&url)
        };
        let started = std::time::Instant::now();
        let shown = ask(&dirs, &slow, true, "ana@example.com\n");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(shown.contains("Could not reach Omukk"));
        assert_eq!(stored(&dirs).map(|a| a.subscribed), Some(false));
        let error = subscribe(
            Some("ana@example.com"),
            &dirs,
            &slow,
            false,
            &mut io::empty(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(error, NewsError::Send(_)));
        assert!(!error.to_string().contains("ana@example.com"));
        drop(kept);
    }

    #[test]
    fn ci_and_the_opt_out_variable_turn_the_question_off() {
        let from = |pairs: &'static [(&'static str, &'static str)]| {
            Settings::from_vars(|name| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).into())
            })
        };
        assert!(!from(&[]).no_prompt);
        assert!(from(&[("CI", "true")]).no_prompt);
        assert!(from(&[(NO_PROMPT_ENV, "1")]).no_prompt);
        assert!(!from(&[("CI", ""), (NO_PROMPT_ENV, "")]).no_prompt);
        assert_eq!(from(&[]).timeout, TIMEOUT);
        assert_eq!(from(&[(URL_ENV, "")]).url, None);
        assert_eq!(
            from(&[(URL_ENV, "https://example.com/x")]).url.as_deref(),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn a_corrupt_answer_file_asks_once_more_and_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs(dir.path());
        let path = answer_file(&dirs).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not = [toml").unwrap();
        let (url, _requests) = server(200);
        assert!(!ask(&dirs, &settings(&url), true, "\n").is_empty());
        assert_eq!(ask(&dirs, &settings(&url), true, "\n"), "");
    }

    #[test]
    fn address_check() {
        assert!(valid_address("ana@example.com"));
        for bad in [
            "",
            "ana",
            "@example.com",
            "ana@",
            "ana@example",
            "a b@example.com",
            "a@@example.com",
            "a@.com",
            "a@example.",
            "a\u{1}b@example.com",
            "a\u{1b}@example.com",
        ] {
            assert!(!valid_address(bad), "{bad}");
        }
    }
}
