// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::structure::Structure;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    PublicCrate,
    PublicSuper,
    Protected,
    Private,
    Internal,
    Unknown,
}

impl Visibility {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::PublicCrate => "public_crate",
            Self::PublicSuper => "public_super",
            Self::Protected => "protected",
            Self::Private => "private",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "public" => Self::Public,
            "public_crate" => Self::PublicCrate,
            "public_super" => Self::PublicSuper,
            "protected" => Self::Protected,
            "private" => Self::Private,
            "internal" => Self::Internal,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
}

/// Mirrors `lib_plugin_abi_v3::lang::SymbolKind`. New variants must
/// be appended (never inserted) and use the matching discriminant
/// from the ABI definition — see lang.rs in lib-plugin-abi-v3 for the
/// ABI compatibility rules. The conversion `From<lang::SymbolKind>`
/// in the parser plugin adapter relies on these staying in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SymbolKind {
    Function = 0,
    Method = 1,
    Class = 2,
    Struct = 3,
    Enum = 4,
    Interface = 5,
    Trait = 6,
    Module = 7,
    Constant = 8,
    Variable = 9,
    Type = 10,
    Property = 11,
    Field = 12,
    Constructor = 13,
    Destructor = 14,
    Operator = 15,
    Macro = 16,
    Namespace = 17,
    Package = 18,
    Unknown = 19,
    /// Module-level import / use / require — see ABI definition.
    Import = 20,
}

impl SymbolKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Module => "module",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Type => "type",
            Self::Property => "property",
            Self::Field => "field",
            Self::Constructor => "constructor",
            Self::Destructor => "destructor",
            Self::Operator => "operator",
            Self::Macro => "macro",
            Self::Namespace => "namespace",
            Self::Package => "package",
            Self::Unknown => "unknown",
            Self::Import => "import",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "function" => Self::Function,
            "method" => Self::Method,
            "class" => Self::Class,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "interface" => Self::Interface,
            "trait" => Self::Trait,
            "module" => Self::Module,
            "constant" => Self::Constant,
            "variable" => Self::Variable,
            "type" => Self::Type,
            "property" => Self::Property,
            "field" => Self::Field,
            "constructor" => Self::Constructor,
            "destructor" => Self::Destructor,
            "operator" => Self::Operator,
            "macro" => Self::Macro,
            "namespace" => Self::Namespace,
            "package" => Self::Package,
            "import" => Self::Import,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

impl Location {
    #[must_use]
    pub fn new(
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
        start_byte: u32,
        end_byte: u32,
    ) -> Self {
        Self {
            start_line,
            start_col,
            end_line,
            end_col,
            start_byte,
            end_byte,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub file_path: PathBuf,
    pub location: Location,
    pub parent_id: Option<SymbolId>,
    pub signature: Option<String>,
    pub description: Option<String>,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub is_entry_point: bool,
    /// The shape of this symbol's syntax — what [`crate::Indexer::clones`] groups on. `None`
    /// for symbols indexed before the structural columns existed, and for the handful whose
    /// byte range does not resolve back to a parse node.
    pub structure: Option<Structure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub id: FileId,
    pub path: PathBuf,
    pub language: Language,
    pub hash: String,
    pub size: u64,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub file: File,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub symbol: Symbol,
    pub score: f32,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub files: Vec<FileNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub path: PathBuf,
    pub language: Language,
    pub symbols: Vec<SymbolNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub children: Vec<SymbolNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub indexed_files: u64,
    pub indexed_symbols: u64,
    pub embedding_dimensions: u32,
    pub embedding_model: String,
    pub last_indexed: Option<String>,
    pub storage_size_bytes: u64,
    /// Which version of the indexing pipeline wrote what is stored — see
    /// [`crate::indexer::PIPELINE_VERSION`]. 0 for an index written before this was recorded.
    pub pipeline_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexProgress {
    pub files_processed: u64,
    pub files_total: u64,
    pub symbols_indexed: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Go,
    C,
    Cpp,
    CSharp,
    Ruby,
    Php,
    Kotlin,
    Scala,
    Swift,
    Bash,
    Lua,
    Sql,
    Json,
    Yaml,
    Toml,
    Xml,
    Html,
    Css,
    Markdown,
    Dockerfile,
    Hcl,
    GraphQL,
    Haskell,
    OCaml,
    Elixir,
    Erlang,
    Zig,
    Unknown,
}

impl Language {
    #[must_use]
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" | "pyw" => Self::Python,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::TypeScript,
            "jsx" => Self::JavaScript,
            "java" => Self::Java,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Self::Cpp,
            "cs" => Self::CSharp,
            "rb" | "rake" | "gemspec" => Self::Ruby,
            "php" => Self::Php,
            "kt" | "kts" => Self::Kotlin,
            "scala" | "sc" => Self::Scala,
            "swift" => Self::Swift,
            "sh" | "bash" | "zsh" => Self::Bash,
            "lua" => Self::Lua,
            "sql" => Self::Sql,
            "json" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "xml" | "xsd" | "xsl" => Self::Xml,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "md" | "markdown" => Self::Markdown,
            "dockerfile" => Self::Dockerfile,
            "tf" | "hcl" => Self::Hcl,
            "graphql" | "gql" => Self::GraphQL,
            "hs" | "lhs" => Self::Haskell,
            "ml" | "mli" => Self::OCaml,
            "ex" | "exs" => Self::Elixir,
            "erl" | "hrl" => Self::Erlang,
            "zig" => Self::Zig,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Java => "java",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Kotlin => "kotlin",
            Self::Scala => "scala",
            Self::Swift => "swift",
            Self::Bash => "bash",
            Self::Lua => "lua",
            Self::Sql => "sql",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Markdown => "markdown",
            Self::Dockerfile => "dockerfile",
            Self::Hcl => "hcl",
            Self::GraphQL => "graphql",
            Self::Haskell => "haskell",
            Self::OCaml => "ocaml",
            Self::Elixir => "elixir",
            Self::Erlang => "erlang",
            Self::Zig => "zig",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "rust" => Self::Rust,
            "python" => Self::Python,
            "javascript" => Self::JavaScript,
            "typescript" => Self::TypeScript,
            "java" => Self::Java,
            "go" => Self::Go,
            "c" => Self::C,
            "cpp" => Self::Cpp,
            "csharp" => Self::CSharp,
            "ruby" => Self::Ruby,
            "php" => Self::Php,
            "kotlin" => Self::Kotlin,
            "scala" => Self::Scala,
            "swift" => Self::Swift,
            "bash" => Self::Bash,
            "lua" => Self::Lua,
            "sql" => Self::Sql,
            "json" => Self::Json,
            "yaml" => Self::Yaml,
            "toml" => Self::Toml,
            "xml" => Self::Xml,
            "html" => Self::Html,
            "css" => Self::Css,
            "markdown" => Self::Markdown,
            "dockerfile" => Self::Dockerfile,
            "hcl" => Self::Hcl,
            "graphql" => Self::GraphQL,
            "haskell" => Self::Haskell,
            "ocaml" => Self::OCaml,
            "elixir" => Self::Elixir,
            "erlang" => Self::Erlang,
            "zig" => Self::Zig,
            _ => Self::Unknown,
        }
    }
}

/// Kind of reference between symbols
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceKind {
    /// Function or method call
    Call,
    /// Type used in signature, variable declaration, or generic
    TypeReference,
    /// Struct/object field access
    FieldAccess,
    /// Import/use statement
    Import,
    /// Trait implementation or class inheritance
    Inheritance,
    /// Macro invocation
    MacroInvocation,
    /// Variable/constant/static reference
    VariableReference,
}

impl ReferenceKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TypeReference => "type",
            Self::FieldAccess => "field",
            Self::Import => "import",
            Self::Inheritance => "inheritance",
            Self::MacroInvocation => "macro",
            Self::VariableReference => "variable",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "call" => Self::Call,
            "type" => Self::TypeReference,
            "field" => Self::FieldAccess,
            "import" => Self::Import,
            "inheritance" => Self::Inheritance,
            "macro" => Self::MacroInvocation,
            "variable" => Self::VariableReference,
            _ => Self::Call,
        }
    }
}

/// Unresolved reference found during parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedReference {
    /// Name of the referenced symbol (may be qualified like "`foo::bar`")
    pub name: String,
    /// Kind of reference
    pub kind: ReferenceKind,
    /// Location where the reference occurs
    pub location: Location,
    /// The containing symbol's index in the parsed symbols list (for resolution)
    pub containing_symbol_index: Option<usize>,
}

/// Resolved reference stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    /// Symbol that contains the reference (caller)
    pub from_symbol_id: SymbolId,
    /// Symbol being referenced (callee)
    pub to_symbol_id: SymbolId,
    /// Kind of reference
    pub kind: ReferenceKind,
    /// Location where the reference occurs
    pub location: Location,
}

/// Symbol usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolUsage {
    pub symbol: Symbol,
    /// Number of times this symbol is referenced
    pub reference_count: u64,
    /// Symbols that reference this one (callers)
    pub callers: Vec<Symbol>,
    /// Symbols that this one references (callees)
    pub callees: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub children: Vec<ParsedSymbol>,
    pub visibility: Visibility,
    /// The shape of this symbol's syntax, filled in by the parser after the analyzer has
    /// named the symbol — see [`crate::structure`]. `None` when the symbol's byte range does
    /// not resolve back to a node, which an analyzer synthesising a symbol can produce.
    #[serde(default)]
    pub structure: Option<Structure>,
}

/// The builders the language analyzers are written against: upstream they came with the plugin
/// ABI's mirror of these types, and moved here when the analyzers stopped crossing an ABI.
impl ParsedSymbol {
    pub fn new(name: impl Into<String>, kind: SymbolKind, location: Location) -> Self {
        Self {
            name: name.into(),
            kind,
            location,
            signature: None,
            doc_comment: None,
            children: vec![],
            visibility: Visibility::Unknown,
            structure: None,
        }
    }

    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    #[must_use]
    pub fn with_doc_comment(mut self, doc: impl Into<String>) -> Self {
        self.doc_comment = Some(doc.into());
        self
    }

    #[must_use]
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    #[must_use]
    pub fn with_children(mut self, children: Vec<ParsedSymbol>) -> Self {
        self.children = children;
        self
    }
}

impl ParsedReference {
    pub fn new(name: impl Into<String>, kind: ReferenceKind, location: Location) -> Self {
        Self {
            name: name.into(),
            kind,
            location,
            containing_symbol_index: None,
        }
    }

    #[must_use]
    pub fn with_containing_symbol(mut self, index: usize) -> Self {
        self.containing_symbol_index = Some(index);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFile {
    pub language: Language,
    pub symbols: Vec<ParsedSymbol>,
    /// Unresolved references found in this file
    pub references: Vec<ParsedReference>,
}
