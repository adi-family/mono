//! Read a crate's `src/**.rs` and reduce it to the types it declares.
//!
//! Every rendered fragment — a field's type, an attribute, a generic parameter list — is a *slice
//! of the original source*, taken by byte span rather than re-printed from tokens. That is the
//! whole trick: `Option<Box<dyn Fn()>>` comes back spelled the way it was written, with no
//! token-stream spacing to undo.

use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;

/// One crate's worth of declarations, in file order.
#[derive(Debug)]
pub(crate) struct CrateTypes {
    /// The crate's package name, e.g. `adi-agents`.
    pub(crate) name: String,
    /// The `description` from its manifest, if it has one.
    pub(crate) description: Option<String>,
    /// Every source file that declares at least one type, sorted by path.
    pub(crate) files: Vec<FileTypes>,
}

/// The declarations of a single source file.
#[derive(Debug)]
pub(crate) struct FileTypes {
    /// Path relative to the crate root, e.g. `src/agent.rs`.
    pub(crate) path: String,
    pub(crate) decls: Vec<Decl>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Struct,
    Enum,
    Alias,
}

impl Kind {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Alias => "type",
        }
    }
}

/// One declared type.
#[derive(Debug)]
pub(crate) struct Decl {
    pub(crate) kind: Kind,
    /// Bare identifier, e.g. `AgentManifest`.
    pub(crate) name: String,
    /// The inline-module path it sits under within its file, e.g. `steps`. Empty at file level.
    pub(crate) module: String,
    /// Generics and any `where` clause, verbatim: `<Args>`, or empty.
    pub(crate) generics: String,
    /// Visibility as written: `pub`, `pub(crate)`, or empty for private.
    pub(crate) vis: String,
    /// Attributes worth keeping — derives, serde, cfg — doc comments and lint knobs dropped.
    pub(crate) attrs: Vec<String>,
    /// The first paragraph of the doc comment, collapsed onto one line.
    pub(crate) doc: String,
    pub(crate) body: Body,
}

impl Decl {
    /// The name as it appears in the page: module-qualified when the type is nested in one.
    pub(crate) fn qualified_name(&self) -> String {
        if self.module.is_empty() {
            self.name.clone()
        } else {
            format!("{}::{}", self.module, self.name)
        }
    }
}

#[derive(Debug)]
pub(crate) enum Body {
    /// `{ a: A, b: B }`
    Named(Vec<Field>),
    /// `(A, B)` — the fields carry no name.
    Tuple(Vec<Field>),
    /// No payload at all.
    Unit,
    Variants(Vec<Variant>),
    /// The right-hand side of a `type X = ...;`.
    Alias(String),
}

#[derive(Debug)]
pub(crate) struct Field {
    pub(crate) vis: String,
    /// Empty for a tuple field.
    pub(crate) name: String,
    pub(crate) ty: String,
    pub(crate) attrs: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Variant {
    pub(crate) name: String,
    pub(crate) body: Body,
    pub(crate) attrs: Vec<String>,
    /// An explicit discriminant, e.g. ` = 1`, or empty.
    pub(crate) discriminant: String,
}

/// The source text of one file, plus the ability to cut spans back out of it.
struct Src {
    text: String,
}

impl Src {
    /// The exact source under `span`, with interior line breaks folded into single spaces so a
    /// type wrapped across three lines still renders as one.
    fn slice(&self, span: proc_macro2::Span) -> String {
        let range = span.byte_range();
        let raw = self.text.get(range).unwrap_or_default();
        collapse_ws(raw)
    }

    /// The source between two byte offsets — used for the stretches syn has no node for, such as
    /// the generics sitting between a type's name and its body.
    fn between(&self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        collapse_ws(self.text.get(start..end).unwrap_or_default())
    }
}

/// Collapse every run of whitespace to a single space and trim the ends.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(ch);
        }
    }
    out
}

/// Scan one crate directory (the one holding its `Cargo.toml`).
pub(crate) fn scan_crate(root: &Path) -> Result<CrateTypes, String> {
    let manifest_path = root.join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&manifest).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let package = manifest.get("package");
    let name = package
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("?")
        })
        .to_string();
    let description = package
        .and_then(|p| p.get("description"))
        .and_then(toml::Value::as_str)
        .map(collapse_ws);

    let mut files = Vec::new();
    for path in rust_files(&root.join("src")) {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed = syn::parse_file(&text).map_err(|e| format!("{rel}: {e}"))?;
        let src = Src { text };
        let mut decls = Vec::new();
        collect(&parsed.items, "", &src, &mut decls);
        if !decls.is_empty() {
            files.push(FileTypes { path: rel, decls });
        }
    }

    Ok(CrateTypes {
        name,
        description,
        files,
    })
}

/// Every `.rs` file under `dir`, recursively, sorted so the output is stable across machines.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Walk a list of items, descending into inline modules, appending what we find to `out`.
fn collect(items: &[syn::Item], module: &str, src: &Src, out: &mut Vec<Decl>) {
    for item in items {
        match item {
            syn::Item::Struct(it) => {
                if let Some(decl) = struct_decl(it, module, src) {
                    out.push(decl);
                }
            }
            syn::Item::Enum(it) => {
                if let Some(decl) = enum_decl(it, module, src) {
                    out.push(decl);
                }
            }
            syn::Item::Type(it) => {
                if let Some(decl) = alias_decl(it, module, src) {
                    out.push(decl);
                }
            }
            syn::Item::Mod(it) => {
                let Some((_, inner)) = &it.content else {
                    continue;
                };
                if is_test_only(&it.attrs) || it.ident == "tests" {
                    continue;
                }
                let nested = if module.is_empty() {
                    it.ident.to_string()
                } else {
                    format!("{module}::{}", it.ident)
                };
                collect(inner, &nested, src, out);
            }
            _ => {}
        }
    }
}

fn struct_decl(it: &syn::ItemStruct, module: &str, src: &Src) -> Option<Decl> {
    if is_test_only(&it.attrs) {
        return None;
    }
    // Generics live between the name and whatever opens the body; a `where` clause rides along.
    let generics_end = match &it.fields {
        syn::Fields::Named(f) => f.brace_token.span.open().byte_range().start,
        syn::Fields::Unnamed(f) => f.paren_token.span.open().byte_range().start,
        syn::Fields::Unit => it.semi_token.map_or(0, |t| t.span().byte_range().start),
    };
    let body = match &it.fields {
        syn::Fields::Named(f) => Body::Named(f.named.iter().map(|f| field(f, src)).collect()),
        syn::Fields::Unnamed(f) => Body::Tuple(f.unnamed.iter().map(|f| field(f, src)).collect()),
        syn::Fields::Unit => Body::Unit,
    };
    // A tuple struct's `where` clause sits *after* the parens, so pick it up separately.
    let trailing_where = match (&it.fields, &it.generics.where_clause) {
        (syn::Fields::Unnamed(f), Some(w)) => {
            format!(
                " {}",
                src.between(
                    f.paren_token.span.close().byte_range().end,
                    w.span().byte_range().end
                )
            )
        }
        _ => String::new(),
    };
    Some(Decl {
        kind: Kind::Struct,
        name: it.ident.to_string(),
        module: module.to_string(),
        generics: format!(
            "{}{trailing_where}",
            src.between(it.ident.span().byte_range().end, generics_end)
        ),
        vis: vis(&it.vis, src),
        attrs: attrs(&it.attrs, src),
        doc: doc(&it.attrs),
        body,
    })
}

fn enum_decl(it: &syn::ItemEnum, module: &str, src: &Src) -> Option<Decl> {
    if is_test_only(&it.attrs) {
        return None;
    }
    let generics_end = it.brace_token.span.open().byte_range().start;
    let variants = it
        .variants
        .iter()
        .map(|v| Variant {
            name: v.ident.to_string(),
            body: match &v.fields {
                syn::Fields::Named(f) => {
                    Body::Named(f.named.iter().map(|f| field(f, src)).collect())
                }
                syn::Fields::Unnamed(f) => {
                    Body::Tuple(f.unnamed.iter().map(|f| field(f, src)).collect())
                }
                syn::Fields::Unit => Body::Unit,
            },
            attrs: attrs(&v.attrs, src),
            discriminant: v
                .discriminant
                .as_ref()
                .map(|(_, expr)| format!(" = {}", src.slice(expr.span())))
                .unwrap_or_default(),
        })
        .collect();
    Some(Decl {
        kind: Kind::Enum,
        name: it.ident.to_string(),
        module: module.to_string(),
        generics: src.between(it.ident.span().byte_range().end, generics_end),
        vis: vis(&it.vis, src),
        attrs: attrs(&it.attrs, src),
        doc: doc(&it.attrs),
        body: Body::Variants(variants),
    })
}

fn alias_decl(it: &syn::ItemType, module: &str, src: &Src) -> Option<Decl> {
    if is_test_only(&it.attrs) {
        return None;
    }
    Some(Decl {
        kind: Kind::Alias,
        name: it.ident.to_string(),
        module: module.to_string(),
        generics: src.between(
            it.ident.span().byte_range().end,
            it.eq_token.span().byte_range().start,
        ),
        vis: vis(&it.vis, src),
        attrs: attrs(&it.attrs, src),
        doc: doc(&it.attrs),
        body: Body::Alias(src.slice(it.ty.span())),
    })
}

fn field(f: &syn::Field, src: &Src) -> Field {
    Field {
        vis: vis(&f.vis, src),
        name: f
            .ident
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        ty: src.slice(f.ty.span()),
        attrs: attrs(&f.attrs, src),
    }
}

fn vis(v: &syn::Visibility, src: &Src) -> String {
    match v {
        syn::Visibility::Inherited => String::new(),
        other => src.slice(other.span()),
    }
}

/// The attributes worth carrying into the page: everything that shapes the type or its wire
/// format (`derive`, `serde`, `schemars`, `repr`, `cfg`, …) and nothing that only speaks to the
/// compiler's lint machinery.
fn attrs(list: &[syn::Attribute], src: &Src) -> Vec<String> {
    const NOISE: [&str; 6] = ["doc", "allow", "warn", "deny", "expect", "rustfmt"];
    list.iter()
        .filter(|a| {
            a.path()
                .get_ident()
                .is_none_or(|id| !NOISE.contains(&id.to_string().as_str()))
        })
        .map(|a| src.slice(a.span()))
        .collect()
}

/// True for an item that only exists in test builds — those are scaffolding, not the data model.
fn is_test_only(list: &[syn::Attribute]) -> bool {
    list.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Path>()
                .is_ok_and(|p| p.is_ident("test"))
    })
}

/// The first paragraph of a doc comment, as one line, with intra-doc links flattened.
fn doc(list: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in list {
        let syn::Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        if !nv.path.is_ident("doc") {
            continue;
        }
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = &nv.value
        else {
            continue;
        };
        let line = s.value();
        let line = line.strip_prefix(' ').unwrap_or(&line).to_string();
        // A blank line ends the first paragraph — the rest is detail the source already holds.
        if line.trim().is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(line);
    }
    flatten_links(&lines.join(" "))
}

/// Flatten rustdoc's links into plain text, because their targets are Rust paths that no markdown
/// reader can follow: ``[`Foo`]``, `[text](Type::method)` and `` [`x`](Self::x) `` all collapse to
/// the text they were showing. Brackets that are not a link — a doc that mentions `[[secrets]]`,
/// say — are left exactly as written.
fn flatten_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let (before, tail) = rest.split_at(open);
        out.push_str(before);
        let Some(close) = tail.find(']') else {
            out.push_str(tail);
            return out;
        };
        let text = &tail[1..close];
        let after = &tail[close + 1..];
        // `[text](target)` — an inline link; keep the text, drop the target.
        if let Some(paren) = after.strip_prefix('(')
            && let Some(end) = paren.find(')')
        {
            out.push_str(text);
            rest = &paren[end + 1..];
            continue;
        }
        // ``[`Foo`]`` — a shortcut intra-doc link; the backticks are the giveaway.
        if text.starts_with('`') && text.ends_with('`') && text.len() > 1 {
            out.push_str(text);
        } else {
            out.push('[');
            out.push_str(text);
            out.push(']');
        }
        rest = after;
    }
    out.push_str(rest);
    out
}
