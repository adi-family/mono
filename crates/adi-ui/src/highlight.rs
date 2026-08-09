//! A small syntax highlighter — everything [`crate::CodeEditor`] knows about language.
//!
//! Deliberately hand-rolled rather than pulled from a crate: `syntect` and friends carry a
//! grammar database that would dwarf the whole wasm bundle, and what a control panel edits
//! is a handful of formats (Rust, TOML, JSON, YAML, TypeScript, shell, SQL, Markdown). This is a
//! scanner per format, good enough to make structure visible — not a parser, and never one
//! that can fail on odd input.
//!
//! Every scanner is total: unterminated strings and comments run to end-of-input and are
//! still emitted, so no file can produce a panic or lose characters. [`highlight`] is a pure
//! function over `&str`, so the render path never has to guard it.
//!
//! This began as a second copy of the same scanner, one here and one in `adi-webapp` against
//! the `adi-css` `.tok--*` classes. They have since converged onto this one, and with them
//! the two palettes: every code view in the app now colours from the `--syn-*` tokens below
//! rather than borrowing the app's status colours (`--online` for a number, `--warn` for a
//! keyword) for a job they were never picked for.

/// What a run of characters is, which is all the renderer needs to pick a colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Plain,
    Comment,
    Str,
    Num,
    /// A key, section header, or Markdown heading — the "left-hand side" of a line.
    Key,
    /// A language keyword or literal (`true`, `null`, `const`, …).
    Kw,
    /// A called function or a property being accessed — the names worth finding when you
    /// skim code, as distinct from the values in `Str`/`Num`.
    Func,
    Punct,
}

impl Tok {
    /// Ink for this token.
    ///
    /// Six colours and two greys, all of them `--syn-*` tokens: correct one in
    /// `styles/tokens.css` and every code view follows. Nothing here sets a *weight* or a
    /// style — the painted layer has to keep the textarea's exact glyph advances, and bold
    /// or italic in a variable font moves them.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Tok::Plain => "text-syn-plain",
            Tok::Comment => "text-syn-comment",
            Tok::Str => "text-syn-str",
            Tok::Num => "text-syn-num",
            Tok::Key => "text-syn-key",
            Tok::Kw => "text-syn-kw",
            Tok::Func => "text-syn-func",
            Tok::Punct => "text-syn-punct",
        }
    }
}

/// The language to scan `path` as, from its extension. Unknown extensions get [`Lang::None`],
/// which emits the text as one plain run — highlighting is an enhancement, never a gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    Toml,
    Json,
    Yaml,
    Ts,
    Rust,
    Sh,
    Sql,
    Md,
    /// Not a format this knows. Emits the text as one plain run — highlighting is an
    /// enhancement, never a gate, so this is also the default.
    #[default]
    None,
}

impl Lang {
    /// Pick a language from a file path's extension.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let ext = path.rsplit('/').next().unwrap_or(path);
        let ext = ext
            .rsplit_once('.')
            .map_or("", |(_, e)| e)
            .to_ascii_lowercase();
        match ext.as_str() {
            "toml" => Lang::Toml,
            "json" => Lang::Json,
            "yaml" | "yml" => Lang::Yaml,
            "ts" | "tsx" | "js" | "mjs" | "jsx" => Lang::Ts,
            "rs" | "rust" => Lang::Rust,
            "sh" | "bash" | "zsh" => Lang::Sh,
            "sql" => Lang::Sql,
            "md" | "markdown" => Lang::Md,
            _ => Lang::None,
        }
    }
}

/// Split `src` into consecutive `(token, text)` runs. Concatenating the texts always
/// reproduces `src` exactly — the renderer relies on that to stay aligned with the textarea.
#[must_use]
pub fn highlight(lang: Lang, src: &str) -> Vec<(Tok, String)> {
    let out = match lang {
        Lang::Toml => scan_toml(src),
        Lang::Json => scan_json(src),
        Lang::Yaml => scan_yaml(src),
        Lang::Ts => scan_ts(src),
        Lang::Rust => scan_rust(src),
        Lang::Sh => scan_sh(src),
        Lang::Sql => scan_sql(src),
        Lang::Md => scan_md(src),
        Lang::None => vec![(Tok::Plain, src.to_string())],
    };
    merge(out)
}

/// Collapse neighbouring runs of the same token into one, so the DOM gets a handful of spans
/// per line instead of one per character.
fn merge(runs: Vec<(Tok, String)>) -> Vec<(Tok, String)> {
    let mut out: Vec<(Tok, String)> = Vec::with_capacity(runs.len());
    for (tok, text) in runs {
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((prev, buf)) if *prev == tok => buf.push_str(&text),
            _ => out.push((tok, text)),
        }
    }
    out
}

/// Consume a quoted string starting at `chars[i]`, honouring backslash escapes. Returns the
/// text and the index just past it; an unterminated string runs to the end of input.
fn take_string(chars: &[char], mut i: usize) -> (String, usize) {
    let quote = chars[i];
    let mut s = String::from(quote);
    i += 1;
    while i < chars.len() {
        let c = chars[i];
        s.push(c);
        i += 1;
        if c == '\\' && i < chars.len() {
            s.push(chars[i]);
            i += 1;
        } else if c == quote {
            break;
        }
    }
    (s, i)
}

/// Consume a run of characters satisfying `pred`, returning it and the index past it.
fn take_while(chars: &[char], mut i: usize, pred: impl Fn(char) -> bool) -> (String, usize) {
    let mut s = String::new();
    while i < chars.len() && pred(chars[i]) {
        s.push(chars[i]);
        i += 1;
    }
    (s, i)
}

fn is_num_start(c: char) -> bool {
    c.is_ascii_digit() || c == '-' || c == '+'
}

fn is_num_body(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' || c == ':'
}

fn scan_json(src: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let (s, next) = take_string(&chars, i);
            // A string is a key when the next non-space character is a colon.
            let mut j = next;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let tok = if chars.get(j) == Some(&':') {
                Tok::Key
            } else {
                Tok::Str
            };
            out.push((tok, s));
            i = next;
        } else if is_num_start(c) && c != '+' {
            let (s, next) = take_while(&chars, i, is_num_body);
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphabetic());
            let tok = match s.as_str() {
                "true" | "false" | "null" => Tok::Kw,
                _ => Tok::Plain,
            };
            out.push((tok, s));
            i = next;
        } else if "{}[],:".contains(c) {
            out.push((Tok::Punct, c.to_string()));
            i += 1;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

fn scan_toml(src: &str) -> Vec<(Tok, String)> {
    let mut out = Vec::new();
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        if indent_len > 0 {
            out.push((Tok::Plain, line[..indent_len].to_string()));
        }
        if trimmed.starts_with('#') {
            out.push((Tok::Comment, trimmed.to_string()));
            continue;
        }
        if trimmed.starts_with('[') {
            out.push((Tok::Key, trimmed.to_string()));
            continue;
        }
        // `key = value`: the name before the first `=` is the key, the rest is scanned.
        match trimmed.split_once('=') {
            Some((k, v)) => {
                out.push((Tok::Key, k.to_string()));
                out.push((Tok::Punct, "=".to_string()));
                out.extend(scan_value(v));
            }
            None => out.push((Tok::Plain, trimmed.to_string())),
        }
    }
    out
}

fn scan_yaml(src: &str) -> Vec<(Tok, String)> {
    let mut out = Vec::new();
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        if indent_len > 0 {
            out.push((Tok::Plain, line[..indent_len].to_string()));
        }
        if trimmed.starts_with('#') {
            out.push((Tok::Comment, trimmed.to_string()));
            continue;
        }
        // A leading `- ` is a sequence marker; what follows can still be `key: value`.
        let rest = if let Some(r) = trimmed.strip_prefix("- ") {
            out.push((Tok::Punct, "- ".to_string()));
            r
        } else {
            trimmed
        };
        match rest.split_once(':') {
            // Only treat it as a key when the colon is followed by a space or end of line —
            // otherwise `http://x` in a bare value would split into a bogus key.
            Some((k, v)) if v.is_empty() || v.starts_with([' ', '\n', '\r']) => {
                out.push((Tok::Key, k.to_string()));
                out.push((Tok::Punct, ":".to_string()));
                out.extend(scan_value(v));
            }
            _ => out.extend(scan_value(rest)),
        }
    }
    out
}

/// Scan the right-hand side of a `key = value` / `key: value` line: a string, number, literal,
/// trailing comment, or plain text.
fn scan_value(v: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = v.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let (s, next) = take_string(&chars, i);
            out.push((Tok::Str, s));
            i = next;
        } else if c == '#' {
            out.push((Tok::Comment, chars[i..].iter().collect::<String>()));
            break;
        } else if c.is_ascii_digit()
            || (is_num_start(c) && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let (s, next) = take_while(&chars, i, is_num_body);
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphanumeric() || c == '_');
            let tok = match s.as_str() {
                "true" | "false" | "null" | "yes" | "no" | "on" | "off" => Tok::Kw,
                _ => Tok::Plain,
            };
            out.push((tok, s));
            i = next;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

const TS_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];

fn scan_ts(src: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            let (s, next) = take_while(&chars, i, |c| c != '\n');
            out.push((Tok::Comment, s));
            i = next;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Block comment: run to the closing `*/`, or to end of input if never closed.
            let mut j = i + 2;
            while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                j += 1;
            }
            let end = (j + 2).min(chars.len());
            out.push((Tok::Comment, chars[i..end].iter().collect::<String>()));
            i = end;
        } else if c == '"' || c == '\'' || c == '`' {
            let (s, next) = take_string(&chars, i);
            out.push((Tok::Str, s));
            i = next;
        } else if c.is_ascii_digit() {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphanumeric() || c == '.');
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let (s, next) = take_while(&chars, i, |c| {
                c.is_ascii_alphanumeric() || c == '_' || c == '$'
            });
            let tok = if TS_KEYWORDS.contains(&s.as_str()) {
                Tok::Kw
            } else {
                // A name is a `Func` when it is called or reached through a dot — that covers
                // both `join(...)` and `process.env`, which is what you scan a module for.
                let mut j = next;
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                let called = chars.get(j) == Some(&'(');
                let member = i > 0 && chars[i - 1] == '.';
                if called || member {
                    Tok::Func
                } else {
                    Tok::Plain
                }
            };
            out.push((tok, s));
            i = next;
        } else if "{}[]();,.:".contains(c) {
            out.push((Tok::Punct, c.to_string()));
            i += 1;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

/// Rust's reserved words, plus the primitive types — in a signature a `usize` reads as part
/// of the language rather than as a name someone chose, which is what colouring it says.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "bool", "break", "char", "const", "continue", "crate", "dyn", "else",
    "enum", "extern", "f32", "f64", "false", "fn", "for", "i8", "i16", "i32", "i64", "i128", "if",
    "impl", "in", "isize", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "str", "struct", "super", "trait", "true", "type", "u8", "u16",
    "u32", "u64", "u128", "unsafe", "use", "usize", "where", "while",
];

/// Rust. Close to [`scan_ts`] in shape, and different in the three places Rust is:
///
/// - **`'` is ambiguous.** `'a'` is a character and `'a` is a lifetime, and the only thing
///   that tells them apart is whether the quote closes.
/// - **Raw strings.** `r#"…"#` has no escapes at all, so it ends at a quote followed by the
///   same number of hashes and nowhere else.
/// - **`name!` is a call.** A macro is the loudest thing on many lines and reads as a
///   function, which is what it is standing in for.
fn scan_rust(src: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            let (s, next) = take_while(&chars, i, |c| c != '\n');
            out.push((Tok::Comment, s));
            i = next;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Rust nests these; not nesting is still total, and a comment that swallows one
            // extra `*/` is a comment either way.
            let mut j = i + 2;
            while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                j += 1;
            }
            let end = (j + 2).min(chars.len());
            out.push((Tok::Comment, chars[i..end].iter().collect::<String>()));
            i = end;
        } else if let Some((s, next)) = take_raw_string(&chars, i) {
            out.push((Tok::Str, s));
            i = next;
        } else if c == '"' {
            let (s, next) = take_string(&chars, i);
            out.push((Tok::Str, s));
            i = next;
        } else if c == '\'' {
            // A character closes on the third char (`'a'`) or is escaped (`'\n'`); anything
            // else that starts with a quote is a lifetime.
            if chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\'') {
                let (s, next) = take_string(&chars, i);
                out.push((Tok::Str, s));
                i = next;
            } else {
                let (name, next) =
                    take_while(&chars, i + 1, |c| c.is_ascii_alphanumeric() || c == '_');
                out.push((Tok::Kw, format!("'{name}")));
                i = next;
            }
        } else if c.is_ascii_digit() {
            let (s, next) = take_while(&chars, i, |c| {
                c.is_ascii_alphanumeric() || c == '.' || c == '_'
            });
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphanumeric() || c == '_');
            out.push((rust_word(&chars, &s, i, next), s));
            i = next;
        } else if "{}[]();,.:&<>|#!?=+-*/".contains(c) {
            out.push((Tok::Punct, c.to_string()));
            i += 1;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

/// What one Rust identifier is: a keyword, something being called, a type, or a plain name.
fn rust_word(chars: &[char], word: &str, start: usize, end: usize) -> Tok {
    if RUST_KEYWORDS.contains(&word) {
        return Tok::Kw;
    }
    let mut j = end;
    while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
        j += 1;
    }
    let called = matches!(chars.get(j), Some('(' | '!'));
    let member = start > 0 && chars[start - 1] == '.';
    // A leading capital is Rust's own convention for a type, and a type is the other name
    // worth finding when you skim a file.
    let ty = word.starts_with(|c: char| c.is_ascii_uppercase());
    if called || member || ty {
        Tok::Func
    } else {
        Tok::Plain
    }
}

/// A raw string at `i` — `r"…"`, `r#"…"#`, `br##"…"##` — and the index past it.
///
/// It ends at a quote followed by exactly the hashes it opened with, and at nothing else:
/// there are no escapes inside, which is the whole point of the form. `None` when this is not
/// a raw string, so the caller can go on trying.
fn take_raw_string(chars: &[char], i: usize) -> Option<(String, usize)> {
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1;
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let hashes = {
        let start = j;
        while chars.get(j) == Some(&'#') {
            j += 1;
        }
        j - start
    };
    if chars.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    while j < chars.len() {
        if chars[j] == '"' && chars[j + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes
        {
            j += 1 + hashes;
            return Some((chars[i..j].iter().collect(), j));
        }
        j += 1;
    }
    // Unterminated: still a string, to the end of input.
    Some((chars[i..].iter().collect(), chars.len()))
}

/// The words that give a shell script its shape. Control flow and the builtins you actually
/// read for — not every command on the system, which would colour the whole script.
const SH_KEYWORDS: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for", "function", "if",
    "in", "local", "read", "return", "select", "shift", "then", "until", "while",
];

/// SQL keywords, lowercase — [`scan_sql`] folds case before matching, so `SELECT` and `select`
/// paint the same. Types (`integer`, `text`, …) are in here too: in a `create table` they read as
/// part of the statement's shape, which is most of what a schema is scanned for.
const SQL_KEYWORDS: &[&str] = &[
    "add",
    "all",
    "alter",
    "and",
    "as",
    "asc",
    "autoincrement",
    "begin",
    "between",
    "blob",
    "by",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "conflict",
    "constraint",
    "create",
    "cross",
    "default",
    "delete",
    "desc",
    "distinct",
    "drop",
    "else",
    "end",
    "escape",
    "except",
    "exists",
    "explain",
    "foreign",
    "from",
    "full",
    "group",
    "having",
    "if",
    "immediate",
    "in",
    "index",
    "inner",
    "insert",
    "int",
    "integer",
    "intersect",
    "into",
    "is",
    "join",
    "key",
    "left",
    "like",
    "limit",
    "not",
    "null",
    "nulls",
    "numeric",
    "offset",
    "on",
    "or",
    "order",
    "outer",
    "pragma",
    "primary",
    "real",
    "references",
    "replace",
    "returning",
    "right",
    "rollback",
    "row",
    "select",
    "set",
    "table",
    "text",
    "then",
    "transaction",
    "trigger",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "values",
    "view",
    "when",
    "where",
    "with",
];

/// Paint SQL: `--` and `/* … */` comments, single-quoted strings (SQL escapes a quote by doubling
/// it, which [`take_string`]'s backslash rule doesn't cover, so this scans them itself),
/// double-quoted identifiers, numbers, keywords, and `?`/`?1` placeholders.
fn scan_sql(src: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            let (s, next) = take_while(&chars, i, |c| c != '\n');
            out.push((Tok::Comment, s));
            i = next;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                j += 1;
            }
            // An unterminated block comment runs to the end of input, like the other scanners.
            let end = (j + 2).min(chars.len());
            out.push((Tok::Comment, chars[i..end].iter().collect()));
            i = end;
        } else if c == '\'' {
            // `'it''s'` is one string: a doubled quote is an escaped quote, not a terminator.
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == '\'' {
                    if chars.get(j + 1) == Some(&'\'') {
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            out.push((Tok::Str, chars[i..j.min(chars.len())].iter().collect()));
            i = j.min(chars.len());
        } else if c == '"' {
            // A quoted identifier — a column or table name, so it reads as a key, not a string.
            let (s, next) = take_string(&chars, i);
            out.push((Tok::Key, s));
            i = next;
        } else if c == '?' {
            // A bind placeholder, `?` or `?1` — worth seeing at a glance.
            let (s, next) = take_while(&chars, i + 1, |c| c.is_ascii_digit());
            out.push((Tok::Func, format!("?{s}")));
            i = next;
        } else if c.is_ascii_digit() {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_digit() || c == '.');
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphanumeric() || c == '_');
            let tok = if SQL_KEYWORDS.contains(&s.to_ascii_lowercase().as_str()) {
                Tok::Kw
            } else {
                Tok::Plain
            };
            out.push((tok, s));
            i = next;
        } else if "(),;*=<>!+-/%|".contains(c) {
            out.push((Tok::Punct, c.to_string()));
            i += 1;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

/// Shell. The one thing worth getting right beyond comments and strings is `$VARIABLE`
/// expansion — a trigger's settings arrive that way, so seeing them stand out is the point.
fn scan_sh(src: &str) -> Vec<(Tok, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' && at_word_start(&chars, i) {
            // A `#` only opens a comment at the start of a word: `${X#pfx}` and `a#b` are not
            // comments.
            let (s, next) = take_while(&chars, i, |c| c != '\n');
            out.push((Tok::Comment, s));
            i = next;
        } else if c == '"' || c == '\'' {
            let (s, next) = take_string(&chars, i);
            out.push((Tok::Str, s));
            i = next;
        } else if c == '$' {
            let (s, next) = take_expansion(&chars, i);
            out.push((Tok::Func, s));
            i = next;
        } else if c.is_ascii_digit() && at_word_start(&chars, i) {
            let (s, next) = take_while(&chars, i, |c| c.is_ascii_alphanumeric() || c == '.');
            out.push((Tok::Num, s));
            i = next;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let (s, next) = take_while(&chars, i, |c| {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            });
            let tok = if SH_KEYWORDS.contains(&s.as_str()) {
                Tok::Kw
            } else if chars.get(next) == Some(&'=') {
                // `name=value` — the assignment target reads like a key.
                Tok::Key
            } else {
                Tok::Plain
            };
            out.push((tok, s));
            i = next;
        } else if "|&;()<>{}".contains(c) {
            out.push((Tok::Punct, c.to_string()));
            i += 1;
        } else {
            out.push((Tok::Plain, c.to_string()));
            i += 1;
        }
    }
    out
}

/// Whether the character at `i` opens a word — i.e. nothing but whitespace or an operator
/// precedes it. Used to tell a comment from a `#` inside a word.
fn at_word_start(chars: &[char], i: usize) -> bool {
    i == 0
        || matches!(
            chars[i - 1],
            ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')'
        )
}

/// Consume a `$` expansion: `$NAME`, `${NAME:-default}`, or `$(command)`. A brace or paren form
/// runs to its closing delimiter, or to end of input if it never closes.
fn take_expansion(chars: &[char], i: usize) -> (String, usize) {
    if matches!(chars.get(i + 1), Some('{' | '(')) {
        {
            let (open, close) = if chars[i + 1] == '{' {
                ('{', '}')
            } else {
                ('(', ')')
            };
            let mut depth = 0usize;
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == open {
                    depth += 1;
                } else if chars[j] == close {
                    depth -= 1;
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                j += 1;
            }
            (chars[i..j].iter().collect(), j)
        }
    } else {
        let (name, next) = take_while(chars, i + 1, |c| c.is_ascii_alphanumeric() || c == '_');
        (format!("${name}"), next)
    }
}

fn scan_md(src: &str) -> Vec<(Tok, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            out.push((Tok::Kw, line.to_string()));
            continue;
        }
        if fenced {
            out.push((Tok::Str, line.to_string()));
        } else if trimmed.starts_with('#') {
            out.push((Tok::Key, line.to_string()));
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let indent_len = line.len() - trimmed.len();
            out.push((Tok::Plain, line[..indent_len].to_string()));
            out.push((Tok::Punct, trimmed[..2].to_string()));
            out.push((Tok::Plain, trimmed[2..].to_string()));
        } else {
            out.push((Tok::Plain, line.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one invariant the overlay depends on: the runs must reproduce the input byte for
    /// byte, or the highlighted layer drifts out of alignment with the textarea beneath it.
    fn assert_lossless(lang: Lang, src: &str) {
        let joined: String = highlight(lang, src).into_iter().map(|(_, s)| s).collect();
        assert_eq!(joined, src, "{lang:?} lost or altered text");
    }

    #[test]
    fn every_language_is_lossless() {
        let samples = [
            (Lang::Toml, "# c\n[sec]\nname = \"Bug\"\nn = 12\n"),
            (Lang::Json, "{\"a\": 1, \"b\": [true, null], \"c\": \"x\"}"),
            (
                Lang::Yaml,
                "# c\nkey: value\nlist:\n  - a: 1\n  - url: http://x\n",
            ),
            (Lang::Ts, "const a = 1; // hi\n/* block */ let s = `t`;\n"),
            (Lang::Sh, "# c\nname=value\nwhile :; do echo \"$X\"; done\n"),
            (Lang::Md, "# H\n\n- item\n\n```ts\ncode\n```\n"),
            (Lang::None, "anything at all"),
        ];
        for (lang, src) in samples {
            assert_lossless(lang, src);
        }
    }

    #[test]
    fn unterminated_constructs_still_terminate() {
        // Each of these would loop or panic if a scanner assumed a closing delimiter.
        assert_lossless(Lang::Ts, "const s = \"never closed");
        assert_lossless(Lang::Ts, "/* never closed");
        assert_lossless(Lang::Toml, "k = 'never closed");
        assert_lossless(Lang::Json, "{\"k\": \"unterminated");
        assert_lossless(Lang::Sh, "echo \"never closed");
        assert_lossless(Lang::Sh, "echo ${NEVER_CLOSED");
        assert_lossless(Lang::Sh, "echo $(never closed");
    }

    /// A trigger's settings reach its code block as `$ADI_*`, so those must be the runs that
    /// stand out — in every spelling a script uses.
    #[test]
    fn shell_expansions_are_highlighted_whole() {
        for src in [
            "echo $ADI_CHAT_ID",
            "echo ${ADI_CHAT_ID:-none}",
            "x=$(date -u)",
        ] {
            let runs = highlight(Lang::Sh, src);
            assert!(
                runs.iter()
                    .any(|(t, s)| *t == Tok::Func && s.starts_with('$')),
                "no expansion found in {src:?}: {runs:?}"
            );
        }
    }

    /// `#` opens a comment only at the start of a word — inside `${VAR#prefix}` it does not.
    #[test]
    fn a_hash_inside_a_word_is_not_a_comment() {
        let runs = highlight(Lang::Sh, "echo ${PATH#/usr}\n");
        assert!(
            !runs.iter().any(|(t, _)| *t == Tok::Comment),
            "misread a parameter expansion as a comment: {runs:?}"
        );
    }

    #[test]
    fn yaml_bare_url_is_not_split_into_a_key() {
        // `http://x` has a colon but no following space — treating it as a key would be wrong.
        let runs = highlight(Lang::Yaml, "- http://x\n");
        assert!(
            !runs.iter().any(|(t, _)| *t == Tok::Key),
            "bare URL misread as a key: {runs:?}"
        );
    }

    #[test]
    fn json_distinguishes_keys_from_string_values() {
        let runs = highlight(Lang::Json, "{\"k\": \"v\"}");
        assert!(runs.iter().any(|(t, s)| *t == Tok::Key && s == "\"k\""));
        assert!(runs.iter().any(|(t, s)| *t == Tok::Str && s == "\"v\""));
    }

    #[test]
    fn language_comes_from_the_extension() {
        assert_eq!(Lang::from_path("a/b/config.toml"), Lang::Toml);
        assert_eq!(Lang::from_path("hive.YAML"), Lang::Yaml);
        assert_eq!(Lang::from_path("x.tsx"), Lang::Ts);
        assert_eq!(Lang::from_path("LICENSE"), Lang::None);
        assert_eq!(Lang::from_path(".gitignore"), Lang::None);
    }
}
