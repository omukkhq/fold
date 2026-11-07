/// One shell command of a command line, as far as a write target can be read from its text: its words, the folder a `cd` before it moved to, and the targets of its output redirects.
#[derive(Debug, Default, PartialEq, Eq)]
struct Simple {
    /// Each word with whether it holds an expansion (`$`, a backtick, `~` at its start or a glob), which the text alone cannot resolve.
    words: Vec<(String, bool)>,
    redirects: Vec<(String, bool)>,
}

/// The paths that `command` names as write targets in the common forms: the targets of `>`, `>>`, `>|`, `&>` and `&>>` redirects, the files of `tee`, `rm`, `touch`, `truncate`, `sed -i`, every operand of `mv`, which removes its sources, the destination of `cp`, `install` and `ln`, and `of=` of `dd`, each with the folder it is relative to: `None` for the session's folder, or the folder of the last `cd` before it. A word that holds an expansion is left out. This reads the text of the command only; it is a net for the common mistakes, never a boundary, as a script or an interpreter writes where its text does not say.
pub fn write_targets(command: &str) -> Vec<(Option<String>, String)> {
    let mut targets = Vec::new();
    let mut moved: Option<String> = None;
    for simple in split(command) {
        let words: Vec<&(String, bool)> = simple.words.iter().collect();
        for (target, expanded) in &simple.redirects {
            if !expanded {
                targets.push((moved.clone(), target.clone()));
            }
        }
        let Some(start) = words.iter().position(|(word, _)| !is_prefix_word(word)) else {
            continue;
        };
        let (name, _) = words[start];
        let args: Vec<&(String, bool)> = words[start + 1..].to_vec();
        if name == "cd" {
            moved = args
                .first()
                .filter(|(_, expanded)| !expanded)
                .map(|(dir, _)| match &moved {
                    Some(before) if !dir.starts_with('/') => format!("{before}/{dir}"),
                    _ => dir.clone(),
                });
            continue;
        }
        let program = name.rsplit('/').next().unwrap_or(name);
        for target in command_targets(program, &args) {
            targets.push((moved.clone(), target));
        }
    }
    targets
}

/// Words before the command itself: a variable assignment, or a program that runs the next word.
fn is_prefix_word(word: &str) -> bool {
    matches!(word, "sudo" | "env" | "command" | "exec" | "nohup" | "time")
        || word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The write targets of one command's arguments.
fn command_targets(program: &str, args: &[&(String, bool)]) -> Vec<String> {
    let mut operands = Vec::new();
    let mut options = Vec::new();
    let mut after_dashes = false;
    let mut skip_value = false;
    for (arg, expanded) in args {
        if skip_value {
            skip_value = false;
            options.push(arg.clone());
            continue;
        }
        if !after_dashes && arg == "--" {
            after_dashes = true;
            continue;
        }
        if !after_dashes && arg.starts_with('-') && arg.len() > 1 {
            // These options take the next word as their value.
            skip_value = matches!(
                (program, arg.as_str()),
                ("truncate", "-s" | "-r")
                    | ("sed", "-e" | "-f")
                    | ("cp" | "mv" | "install" | "ln", "-t" | "-S")
                    | ("install", "-m" | "-o" | "-g")
            );
            options.push(arg.clone());
            continue;
        }
        if !expanded {
            operands.push(arg.clone());
        } else {
            operands.push(String::new());
        }
    }
    let plain = |list: Vec<String>| list.into_iter().filter(|word| !word.is_empty()).collect();
    match program {
        // `mv` removes each source as it moves it, so every operand is changed.
        "tee" | "rm" | "rmdir" | "touch" | "truncate" | "shred" | "unlink" | "mv" => plain(
            operands
                .into_iter()
                .chain(options_value(&options, "-t"))
                .collect(),
        ),
        "cp" | "install" | "ln" => {
            if let Some(dir) = options_value(&options, "-t") {
                return plain(vec![dir]);
            }
            if operands.len() >= 2 {
                plain(operands.last().cloned().into_iter().collect())
            } else {
                Vec::new()
            }
        }
        "sed" => {
            let in_place = options
                .iter()
                .any(|option| option.starts_with("-i") || option.starts_with("--in-place"));
            if !in_place {
                return Vec::new();
            }
            let scripted = options
                .iter()
                .any(|option| option == "-e" || option == "-f");
            plain(operands.into_iter().skip(usize::from(!scripted)).collect())
        }
        "dd" => plain(
            operands
                .iter()
                .filter_map(|operand| operand.strip_prefix("of="))
                .map(str::to_owned)
                .collect(),
        ),
        _ => Vec::new(),
    }
}

/// The value that follows the option `name` among `options`, as [`command_targets`] collected them, or the value of its `--target-directory=` form.
fn options_value(options: &[String], name: &str) -> Option<String> {
    options
        .iter()
        .position(|option| option == name)
        .and_then(|at| options.get(at + 1).cloned())
        .or_else(|| {
            options
                .iter()
                .find_map(|option| option.strip_prefix("--target-directory="))
                .map(str::to_owned)
        })
}

/// The simple commands of `command`, split at `;`, `&`, `|`, newlines and parentheses outside quotes.
fn split(command: &str) -> Vec<Simple> {
    let mut commands = Vec::new();
    let mut current = Simple::default();
    let mut word = String::new();
    let mut in_word = false;
    let mut expanded = false;
    // The next word is the target of an output redirect (`Some(true)`) or the file of an input redirect (`Some(false)`), which is read, not written.
    let mut redirect: Option<bool> = None;
    let mut chars = command.chars().peekable();
    let finish_word = |current: &mut Simple,
                       word: &mut String,
                       in_word: &mut bool,
                       expanded: &mut bool,
                       redirect: &mut Option<bool>| {
        if *in_word {
            let taken = (std::mem::take(word), *expanded);
            match redirect.take() {
                Some(true) => current.redirects.push(taken),
                Some(false) => {}
                None => current.words.push(taken),
            }
        }
        *in_word = false;
        *expanded = false;
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    word.push(inner);
                }
            }
            '"' => {
                in_word = true;
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                word.push(next);
                            }
                        }
                        '$' | '`' => {
                            expanded = true;
                            word.push(inner);
                        }
                        _ => word.push(inner),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            '$' | '`' | '*' | '?' | '[' | '{' => {
                in_word = true;
                expanded = true;
                word.push(c);
            }
            '~' if !in_word => {
                in_word = true;
                expanded = true;
                word.push(c);
            }
            '>' => {
                // A file descriptor number such as `2` before `>` is part of the operator.
                if in_word && !expanded && word.chars().all(|d| d.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                } else {
                    finish_word(
                        &mut current,
                        &mut word,
                        &mut in_word,
                        &mut expanded,
                        &mut redirect,
                    );
                }
                if matches!(chars.peek(), Some('>' | '|')) {
                    chars.next();
                }
                if chars.peek() == Some(&'&') {
                    // `>&2` duplicates a descriptor and opens no file.
                    chars.next();
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                    continue;
                }
                redirect = Some(true);
            }
            '<' => {
                finish_word(
                    &mut current,
                    &mut word,
                    &mut in_word,
                    &mut expanded,
                    &mut redirect,
                );
                while chars.peek() == Some(&'<') {
                    chars.next();
                }
                redirect = Some(false);
            }
            '&' if chars.peek() == Some(&'>') => {
                finish_word(
                    &mut current,
                    &mut word,
                    &mut in_word,
                    &mut expanded,
                    &mut redirect,
                );
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                redirect = Some(true);
            }
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                finish_word(
                    &mut current,
                    &mut word,
                    &mut in_word,
                    &mut expanded,
                    &mut redirect,
                );
                redirect = None;
                commands.push(std::mem::take(&mut current));
            }
            c if c.is_whitespace() => {
                finish_word(
                    &mut current,
                    &mut word,
                    &mut in_word,
                    &mut expanded,
                    &mut redirect,
                );
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
    }
    finish_word(
        &mut current,
        &mut word,
        &mut in_word,
        &mut expanded,
        &mut redirect,
    );
    commands.push(current);
    commands
        .into_iter()
        .filter(|simple| !simple.words.is_empty() || !simple.redirects.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(command: &str) -> Vec<String> {
        write_targets(command)
            .into_iter()
            .map(|(dir, path)| match dir {
                Some(dir) => format!("{dir}:{path}"),
                None => path,
            })
            .collect()
    }

    #[test]
    fn redirects_name_their_targets_and_never_a_descriptor_or_an_input() {
        assert_eq!(targets("echo hi > design/a.md"), ["design/a.md"]);
        assert_eq!(targets("echo hi >>design/a.md"), ["design/a.md"]);
        assert_eq!(
            targets("make 2> log.txt >| out.txt"),
            ["log.txt", "out.txt"]
        );
        assert_eq!(
            targets("make &> all.txt; make &>> more.txt"),
            ["all.txt", "more.txt"]
        );
        assert_eq!(targets("make 2>&1 | tee -a 'my log.txt'"), ["my log.txt"]);
        assert_eq!(targets("cat < design/a.md"), Vec::<String>::new());
        assert_eq!(targets("echo > \"$OUT\""), Vec::<String>::new());
        assert_eq!(targets("echo > ~/notes.md"), Vec::<String>::new());
        assert_eq!(targets("echo '>' design/a.md"), Vec::<String>::new());
    }

    #[test]
    fn file_commands_name_the_files_they_change() {
        assert_eq!(
            targets("rm -rf design/old docs/x.pdf"),
            ["design/old", "docs/x.pdf"]
        );
        assert_eq!(targets("touch a b"), ["a", "b"]);
        assert_eq!(targets("truncate -s 0 design/a.md"), ["design/a.md"]);
        assert_eq!(targets("cp -r src/a design/"), ["design/"]);
        assert_eq!(targets("mv a.md b.md"), ["a.md", "b.md"]);
        // Moving a shared file away removes it from the project folder.
        assert_eq!(
            targets("mv -f design/a.md design/b.md src/"),
            ["design/a.md", "design/b.md", "src/"]
        );
        assert_eq!(targets("mv -t src design/a.md"), ["design/a.md", "src"]);
        assert_eq!(targets("cp -t design a b"), ["design"]);
        assert_eq!(targets("install --target-directory=design a"), ["design"]);
        assert_eq!(targets("ln -sf ../x design/x"), ["design/x"]);
        assert_eq!(targets("sed -i 's/a/b/' design/a.md"), ["design/a.md"]);
        assert_eq!(targets("sed -i.bak -e s/a/b/ design/a.md"), ["design/a.md"]);
        assert_eq!(targets("sed 's/a/b/' design/a.md"), Vec::<String>::new());
        assert_eq!(
            targets("dd if=/dev/zero of=design/a.bin bs=1"),
            ["design/a.bin"]
        );
        assert_eq!(targets("sudo FOO=1 /bin/rm design/a"), ["design/a"]);
        assert_eq!(
            targets("cat design/a.md && ls design"),
            Vec::<String>::new()
        );
        assert_eq!(targets("rm $FILES"), Vec::<String>::new());
    }

    #[test]
    fn a_cd_before_a_command_names_the_folder_its_targets_are_in() {
        assert_eq!(
            targets("cd '/app/.fold/ws/t-1' && echo x > design/a.md"),
            ["/app/.fold/ws/t-1:design/a.md"]
        );
        assert_eq!(targets("cd sub; cd deeper; rm a"), ["sub/deeper:a"]);
    }
}
