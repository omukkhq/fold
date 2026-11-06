use std::io::{BufRead, IsTerminal, Write};

use fold_core::{FoldError, codes};

use super::remote::RemoteError;

/// How a command that discards the human's work gets its yes: `--yes` gives it at once, a terminal asks, and anything else refuses, so a script never discards work by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub yes: bool,
    pub interactive: bool,
}

impl Consent {
    /// The consent of a command run with or without `--yes`, which asks only when stdin and stdout are both terminals.
    pub fn of(yes: bool) -> Self {
        Self {
            yes,
            interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        }
    }

    /// True when the human agrees to `question`. Only `y` or `yes`, in any case, agrees; Enter and the end of input do not. Without `--yes` and a terminal, the command is refused with the flag that runs it, named by `command`.
    pub fn ask(
        self,
        question: &str,
        command: &str,
        input: &mut impl BufRead,
        out: &mut impl Write,
    ) -> Result<bool, RemoteError> {
        if self.yes {
            return Ok(true);
        }
        if !self.interactive {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{command} discards work, so it asks first, and no terminal can answer"),
            )
            .with_hint(format!(
                "run `{command} --yes` to go on without the question"
            ))
            .into());
        }
        write!(out, "{question} [y/N] ")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(false);
        }
        Ok(matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(consent: Consent, typed: &str) -> (Result<bool, RemoteError>, String) {
        let mut out = Vec::new();
        let answer = consent.ask(
            "drop stash@{0}?",
            "fld stash drop",
            &mut typed.as_bytes(),
            &mut out,
        );
        (answer, String::from_utf8(out).unwrap())
    }

    #[test]
    fn only_yes_agrees_and_no_terminal_is_refused_without_the_flag() {
        let terminal = Consent {
            yes: false,
            interactive: true,
        };
        for (typed, agrees) in [
            ("y\n", true),
            ("YES\n", true),
            ("\n", false),
            ("n\n", false),
            ("maybe\n", false),
            ("", false),
        ] {
            let (answer, out) = ask(terminal, typed);
            assert_eq!(answer.unwrap(), agrees, "{typed:?}");
            assert!(out.starts_with("drop stash@{0}? [y/N] "), "{out}");
        }
        let (answer, out) = ask(
            Consent {
                yes: true,
                interactive: false,
            },
            "",
        );
        assert!(answer.unwrap());
        assert!(out.is_empty());
        let (answer, out) = ask(
            Consent {
                yes: false,
                interactive: false,
            },
            "y\n",
        );
        let refused = answer.unwrap_err();
        assert_eq!(refused.code(), Some("invalid_request"));
        assert_eq!(
            refused.hint(),
            Some("run `fld stash drop --yes` to go on without the question")
        );
        assert!(out.is_empty());
    }
}
