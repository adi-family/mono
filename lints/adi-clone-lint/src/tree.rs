// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! HIR lowered into a flat arena that path extraction can walk in both directions.
//!
//! rustc's HIR is a tree of borrowed references with no parent pointers, and an AST path is by
//! definition a walk *up* to a common ancestor and back down. So the first thing this does is
//! copy the shape into an arena where every node knows its parent, its depth, and the span of
//! leaves underneath it.
//!
//! Three properties of the arena are what the rest of the lint is built on:
//!
//! * **Leaves are numbered left to right**, and every subtree therefore covers a *contiguous*
//!   range of them. That is what lets a fragment's path bag be a range query instead of a
//!   re-walk — see [`crate::paths`].
//! * **Identity is separated from shape.** A node's `label` is its syntax with every name taken
//!   out; names live in `name`, and variable references live in `terminal`. A fingerprint
//!   chooses how much of that to include, so one arena serves both exact and loose matching.
//! * **Locals are recorded as their `HirId`**, not their spelling. rustc has already resolved
//!   every identifier, so two bindings are the same binding iff their ids match — which makes
//!   the renaming in [`crate::fingerprint`] a proof rather than a guess.
//!
//! Working on HIR rather than the source tree also means the sugar is already gone: `for`,
//! `while let`, `?` and `if let` are all desugared to loops and matches before this sees them,
//! so two fragments that differ only in which sugar they were written with still match.

use rustc_ast::ast::{AssignOpKind, LitKind};
use rustc_hir::def::Res;
use rustc_hir::{
    BinOpKind, Block, Expr, ExprKind, LetStmt, Pat, PatKind, QPath, Stmt, StmtKind, UnOp,
};
use rustc_span::{ExpnKind, Span, Symbol};

/// Index into [`Tree::nodes`].
pub type NodeId = u32;

/// Whether a span came from a **macro**, as opposed to a compiler desugaring.
///
/// `Span::from_expansion` cannot be used for this. It is true of desugarings as well, and HIR is
/// what the compiler produces *after* desugaring — every `for`, every `?`, every `.await` is a
/// synthesised `match` or `loop` carrying a desugaring context. Skipping those would skip most
/// of the control flow in the crate and leave the lint quietly reporting almost nothing.
///
/// Macro expansions genuinely do have to go: two calls to the same macro expand to the same
/// tree, so every macro would otherwise be a clone of itself at every call site.
#[must_use]
pub fn from_macro(mut span: Span) -> bool {
    // Walk the whole chain, not just the outermost frame: a desugaring can sit inside a macro
    // expansion, and it is the macro anywhere in the chain that disqualifies the span.
    while !span.ctxt().is_root() {
        let data = span.ctxt().outer_expn_data();
        if matches!(data.kind, ExpnKind::Macro(..)) {
            return true;
        }
        span = data.call_site;
    }
    false
}

/// What a leaf refers to, once rustc's resolution is taken into account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    /// A local binding or a use of one, identified by the `HirId` of the binding itself.
    ///
    /// The id and not the name: `Res::Local` has already told us which binding this is, so a
    /// shadowed `x` and the `x` shadowing it are correctly two different terminals.
    Local(rustc_hir::HirId),
    /// A resolved item — a function, const, static or unit variant. Two fragments calling
    /// *different* functions are not clones, so this stays in the fingerprint by name.
    Item(rustc_span::def_id::DefId),
    /// A literal, reduced to its class. Type-2 clones are allowed to differ in literal values,
    /// so `1` and `4096` are the same terminal and the difference is reported separately.
    Lit(&'static str),
    /// A path named but not yet resolved — `<T>::new`, whose target type checking decides.
    Named(Symbol),
    /// A path that resolved to something else, or not at all.
    Other,
}

/// One node of the lowered tree.
#[derive(Debug, Clone)]
pub struct Node {
    /// The syntax kind with all names removed — `"binary:+"`, `"if"`, `"call"`.
    pub label: &'static str,
    /// A field or method name. Held apart from `label` because it is real evidence for an
    /// exact match (`.len()` is not `.capacity()`) and noise for a loose one.
    pub name: Option<Symbol>,
    /// Set on leaves only.
    pub terminal: Option<Terminal>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub depth: u32,
    pub span: Span,
    /// The HIR node this came from, where there is one. Diagnostics are emitted against it so
    /// that an `#[allow]` on the statement or the enclosing item is honoured — a report-only
    /// lint that cannot be switched off locally is a lint people switch off globally.
    pub hir_id: Option<rustc_hir::HirId>,
    /// Nodes in this subtree, itself included. The size floor a fragment must clear.
    pub size: u32,
    /// Leaves under this node, as the half-open range `leaf_lo..leaf_hi`.
    pub leaf_lo: u32,
    pub leaf_hi: u32,
}

/// One function body, lowered.
#[derive(Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// Node ids of the leaves, left to right. A subtree's slice of this is `leaf_lo..leaf_hi`.
    pub leaves: Vec<NodeId>,
}

impl Tree {
    /// Lower a body's outermost expression.
    ///
    /// Returns `None` for a body that is entirely macro-generated, which would otherwise make
    /// every expansion of the same macro a clone of every other.
    pub fn build(body: &Expr<'_>) -> Option<Self> {
        if from_macro(body.span) {
            return None;
        }
        let mut tree = Tree::default();
        tree.expr(body, None, 0)?;
        Some(tree)
    }

    /// Append a node, returning its id. Children and the leaf range are filled in by the caller
    /// once its subtree is built.
    fn push(
        &mut self,
        label: &'static str,
        name: Option<Symbol>,
        terminal: Option<Terminal>,
        parent: Option<NodeId>,
        depth: u32,
        span: Span,
        hir_id: Option<rustc_hir::HirId>,
    ) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            label,
            name,
            terminal,
            parent,
            children: Vec::new(),
            depth,
            span,
            hir_id,
            size: 1,
            leaf_lo: self.leaves.len() as u32,
            leaf_hi: self.leaves.len() as u32,
        });
        id
    }

    /// Close a node once its children are in: record its subtree size and leaf range, and
    /// register it as a leaf if nothing was added under it.
    fn close(&mut self, id: NodeId) {
        let lo = self.nodes[id as usize].leaf_lo;
        if self.nodes[id as usize].children.is_empty() {
            self.leaves.push(id);
        }
        let hi = self.leaves.len() as u32;
        let size = 1 + self.nodes[id as usize]
            .children
            .iter()
            .map(|&c| self.nodes[c as usize].size)
            .sum::<u32>();
        let node = &mut self.nodes[id as usize];
        node.leaf_lo = lo;
        node.leaf_hi = hi;
        node.size = size;
    }

    fn attach(&mut self, parent: Option<NodeId>, child: NodeId) {
        if let Some(parent) = parent {
            self.nodes[parent as usize].children.push(child);
        }
    }

    /// Lower an expression and everything under it.
    ///
    /// Returns `None` when the expression came from a macro. That propagates: a fragment
    /// containing expanded code is dropped rather than half-lowered, because a half-lowered
    /// subtree has a shape that no source the reader can see actually has.
    fn expr(&mut self, expr: &Expr<'_>, parent: Option<NodeId>, depth: u32) -> Option<NodeId> {
        if from_macro(expr.span) {
            return None;
        }

        let (label, name, terminal) = describe(expr);
        let id = self.push(label, name, terminal, parent, depth, expr.span, Some(expr.hir_id));
        self.attach(parent, id);

        let next = depth + 1;
        match expr.kind {
            ExprKind::Call(callee, args) => {
                self.expr(callee, Some(id), next);
                for arg in args {
                    self.expr(arg, Some(id), next);
                }
            }
            ExprKind::MethodCall(_, receiver, args, _) => {
                self.expr(receiver, Some(id), next);
                for arg in args {
                    self.expr(arg, Some(id), next);
                }
            }
            ExprKind::Binary(_, lhs, rhs)
            | ExprKind::Assign(lhs, rhs, _)
            | ExprKind::AssignOp(_, lhs, rhs)
            | ExprKind::Index(lhs, rhs, _) => {
                self.expr(lhs, Some(id), next);
                self.expr(rhs, Some(id), next);
            }
            ExprKind::Unary(_, inner)
            | ExprKind::Cast(inner, _)
            | ExprKind::Type(inner, _)
            | ExprKind::DropTemps(inner)
            | ExprKind::AddrOf(_, _, inner)
            | ExprKind::Field(inner, _)
            | ExprKind::Become(inner)
            | ExprKind::Yield(inner, _) => {
                self.expr(inner, Some(id), next);
            }
            ExprKind::Array(items) | ExprKind::Tup(items) => {
                for item in items {
                    self.expr(item, Some(id), next);
                }
            }
            ExprKind::If(cond, then, otherwise) => {
                self.expr(cond, Some(id), next);
                self.expr(then, Some(id), next);
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise, Some(id), next);
                }
            }
            ExprKind::Match(scrutinee, arms, _) => {
                self.expr(scrutinee, Some(id), next);
                for arm in arms {
                    let arm_id = self.push("arm", None, None, Some(id), next, arm.span, Some(arm.hir_id));
                    self.attach(Some(id), arm_id);
                    self.pat(arm.pat, Some(arm_id), next + 1);
                    if let Some(guard) = arm.guard {
                        self.expr(guard, Some(arm_id), next + 1);
                    }
                    self.expr(arm.body, Some(arm_id), next + 1);
                    self.close(arm_id);
                }
            }
            ExprKind::Loop(block, ..) => {
                self.block(block, Some(id), next);
            }
            ExprKind::Block(block, _) => {
                self.block(block, Some(id), next);
            }
            ExprKind::Let(let_expr) => {
                self.pat(let_expr.pat, Some(id), next);
                self.expr(let_expr.init, Some(id), next);
            }
            ExprKind::Ret(value) => {
                if let Some(value) = value {
                    self.expr(value, Some(id), next);
                }
            }
            ExprKind::Break(_, value) => {
                if let Some(value) = value {
                    self.expr(value, Some(id), next);
                }
            }
            ExprKind::Struct(_, fields, tail) => {
                for field in fields {
                    let field_id = self.push(
                        "field-init",
                        Some(field.ident.name),
                        None,
                        Some(id),
                        next,
                        field.span,
                        Some(field.hir_id),
                    );
                    self.attach(Some(id), field_id);
                    self.expr(field.expr, Some(field_id), next + 1);
                    self.close(field_id);
                }
                if let rustc_hir::StructTailExpr::Base(base) = tail {
                    self.expr(base, Some(id), next);
                }
            }
            ExprKind::Repeat(value, len) => {
                self.expr(value, Some(id), next);
                let _ = len;
            }
            ExprKind::Closure(closure) => {
                // The closure's body is a separate `Body`; the lint visits it in its own right,
                // so descending here would fingerprint it twice and report it as its own clone.
                let _ = closure;
            }
            // Paths, literals, `continue`, inline asm, and anything this match does not name
            // are leaves. An unnamed variant costs recall inside it, never correctness.
            _ => {}
        }

        self.close(id);
        Some(id)
    }

    fn block(&mut self, block: &Block<'_>, parent: Option<NodeId>, depth: u32) -> Option<NodeId> {
        if from_macro(block.span) {
            return None;
        }
        let id = self.push("block", None, None, parent, depth, block.span, Some(block.hir_id));
        self.attach(parent, id);

        for stmt in block.stmts {
            self.stmt(stmt, Some(id), depth + 1);
        }
        if let Some(tail) = block.expr {
            self.expr(tail, Some(id), depth + 1);
        }

        self.close(id);
        Some(id)
    }

    fn stmt(&mut self, stmt: &Stmt<'_>, parent: Option<NodeId>, depth: u32) -> Option<NodeId> {
        if from_macro(stmt.span) {
            return None;
        }
        match stmt.kind {
            StmtKind::Let(local) => self.let_stmt(local, parent, depth),
            StmtKind::Expr(expr) | StmtKind::Semi(expr) => self.expr(expr, parent, depth),
            // A nested item is visited as its own body.
            StmtKind::Item(_) => None,
        }
    }

    fn let_stmt(&mut self, local: &LetStmt<'_>, parent: Option<NodeId>, depth: u32) -> Option<NodeId> {
        let id = self.push("let", None, None, parent, depth, local.span, Some(local.hir_id));
        self.attach(parent, id);

        self.pat(local.pat, Some(id), depth + 1);
        if local.ty.is_some() {
            // Opaque: a type annotation is structure, but two clones are allowed to differ in
            // their types, so descending into it would only ever split a real pair apart.
            let ty_id = self.push("ty", None, None, Some(id), depth + 1, local.span, None);
            self.attach(Some(id), ty_id);
            self.close(ty_id);
        }
        if let Some(init) = local.init {
            self.expr(init, Some(id), depth + 1);
        }
        if let Some(els) = local.els {
            self.block(els, Some(id), depth + 1);
        }

        self.close(id);
        Some(id)
    }

    fn pat(&mut self, pat: &Pat<'_>, parent: Option<NodeId>, depth: u32) -> Option<NodeId> {
        if from_macro(pat.span) {
            return None;
        }

        let (label, terminal) = match pat.kind {
            // The binding site is where a local's `HirId` is minted; every later use resolves
            // back to this same id, which is what ties a renaming together.
            PatKind::Binding(_, hir_id, _, _) => ("pat-binding", Some(Terminal::Local(hir_id))),
            PatKind::Wild => ("pat-wild", None),
            PatKind::Struct(..) => ("pat-struct", None),
            PatKind::TupleStruct(..) => ("pat-tuple-struct", None),
            PatKind::Tuple(..) => ("pat-tuple", None),
            PatKind::Or(..) => ("pat-or", None),
            PatKind::Ref(..) => ("pat-ref", None),
            PatKind::Slice(..) => ("pat-slice", None),
            _ => ("pat", None),
        };

        let id = self.push(label, None, terminal, parent, depth, pat.span, Some(pat.hir_id));
        self.attach(parent, id);

        let next = depth + 1;
        match pat.kind {
            PatKind::Binding(_, _, _, Some(sub)) => {
                self.pat(sub, Some(id), next);
            }
            PatKind::Struct(_, fields, _) => {
                for field in fields {
                    self.pat(field.pat, Some(id), next);
                }
            }
            PatKind::TupleStruct(_, pats, _)
            | PatKind::Tuple(pats, _)
            | PatKind::Or(pats)
            | PatKind::Slice(pats, _, _) => {
                for p in pats {
                    self.pat(p, Some(id), next);
                }
            }
            PatKind::Ref(inner, _, _) | PatKind::Box(inner) => {
                self.pat(inner, Some(id), next);
            }
            _ => {}
        }

        self.close(id);
        Some(id)
    }
}

/// A node's label, its droppable name, and its terminal if it is a leaf.
fn describe(expr: &Expr<'_>) -> (&'static str, Option<Symbol>, Option<Terminal>) {
    match expr.kind {
        ExprKind::Call(..) => ("call", None, None),
        ExprKind::MethodCall(segment, ..) => ("method", Some(segment.ident.name), None),
        ExprKind::Binary(op, ..) => (binop(op.node), None, None),
        ExprKind::AssignOp(op, ..) => (assign_op(op.node), None, None),
        ExprKind::Unary(op, _) => (
            match op {
                UnOp::Deref => "unary:*",
                UnOp::Not => "unary:!",
                UnOp::Neg => "unary:-",
            },
            None,
            None,
        ),
        ExprKind::Lit(lit) => ("lit", None, Some(Terminal::Lit(lit_class(&lit.node)))),
        ExprKind::Path(ref qpath) => ("path", None, Some(path_terminal(qpath))),
        ExprKind::Field(_, ident) => ("field", Some(ident.name), None),
        ExprKind::Index(..) => ("index", None, None),
        ExprKind::Cast(..) => ("cast", None, None),
        ExprKind::Type(..) => ("type", None, None),
        ExprKind::DropTemps(..) => ("drop-temps", None, None),
        ExprKind::AddrOf(..) => ("addr-of", None, None),
        ExprKind::If(..) => ("if", None, None),
        ExprKind::Match(_, _, source) => (
            match source {
                rustc_hir::MatchSource::Normal => "match",
                // Desugared control flow, kept apart from a hand-written `match` so that a
                // `?` and a spelled-out match on the same value do not read as identical.
                rustc_hir::MatchSource::TryDesugar(_) => "match:try",
                rustc_hir::MatchSource::AwaitDesugar => "match:await",
                _ => "match:desugar",
            },
            None,
            None,
        ),
        ExprKind::Loop(_, _, source, _) => (
            match source {
                rustc_hir::LoopSource::Loop => "loop",
                rustc_hir::LoopSource::While => "while",
                rustc_hir::LoopSource::ForLoop => "for",
            },
            None,
            None,
        ),
        ExprKind::Block(..) => ("block-expr", None, None),
        ExprKind::Assign(..) => ("assign", None, None),
        ExprKind::Array(..) => ("array", None, None),
        ExprKind::Tup(..) => ("tuple", None, None),
        ExprKind::Struct(..) => ("struct", None, None),
        ExprKind::Repeat(..) => ("repeat", None, None),
        ExprKind::Ret(..) => ("return", None, None),
        ExprKind::Become(..) => ("become", None, None),
        ExprKind::Break(..) => ("break", None, None),
        ExprKind::Continue(..) => ("continue", None, None),
        ExprKind::Let(..) => ("let-expr", None, None),
        ExprKind::Closure(..) => ("closure", None, None),
        ExprKind::Yield(..) => ("yield", None, None),
        ExprKind::ConstBlock(..) => ("const-block", None, None),
        ExprKind::InlineAsm(..) => ("asm", None, None),
        ExprKind::OffsetOf(..) => ("offset-of", None, None),
        _ => ("expr", None, None),
    }
}

/// What a path leaf refers to, from rustc's own resolution.
fn path_terminal(qpath: &QPath<'_>) -> Terminal {
    match qpath {
        QPath::Resolved(_, path) => match path.res {
            Res::Local(hir_id) => Terminal::Local(hir_id),
            Res::Def(_, def_id) => Terminal::Item(def_id),
            Res::SelfCtor(def_id) => Terminal::Item(def_id),
            Res::SelfTyAlias { alias_to, .. } => Terminal::Item(alias_to),
            _ => Terminal::Other,
        },
        // `<T>::new` and friends are resolved later, during type checking, so there is no
        // `DefId` to key on here. The segment name is still real identity — it is what keeps
        // `Vec::new` apart from `HashMap::with_capacity` — so keep that rather than give up.
        QPath::TypeRelative(_, segment) => Terminal::Named(segment.ident.name),
    }
}

fn lit_class(kind: &LitKind) -> &'static str {
    match kind {
        LitKind::Str(..) => "str",
        LitKind::ByteStr(..) => "bytestr",
        LitKind::CStr(..) => "cstr",
        LitKind::Byte(_) => "byte",
        LitKind::Char(_) => "char",
        LitKind::Int(..) => "int",
        LitKind::Float(..) => "float",
        LitKind::Bool(_) => "bool",
        LitKind::Err(_) => "err",
    }
}

fn binop(op: BinOpKind) -> &'static str {
    match op {
        BinOpKind::Add => "binary:+",
        BinOpKind::Sub => "binary:-",
        BinOpKind::Mul => "binary:*",
        BinOpKind::Div => "binary:/",
        BinOpKind::Rem => "binary:%",
        BinOpKind::And => "binary:&&",
        BinOpKind::Or => "binary:||",
        BinOpKind::BitXor => "binary:^",
        BinOpKind::BitAnd => "binary:&",
        BinOpKind::BitOr => "binary:|",
        BinOpKind::Shl => "binary:<<",
        BinOpKind::Shr => "binary:>>",
        BinOpKind::Eq => "binary:==",
        BinOpKind::Lt => "binary:<",
        BinOpKind::Le => "binary:<=",
        BinOpKind::Ne => "binary:!=",
        BinOpKind::Ge => "binary:>=",
        BinOpKind::Gt => "binary:>",
    }
}

fn assign_op(op: AssignOpKind) -> &'static str {
    match op {
        AssignOpKind::AddAssign => "assign:+=",
        AssignOpKind::SubAssign => "assign:-=",
        AssignOpKind::MulAssign => "assign:*=",
        AssignOpKind::DivAssign => "assign:/=",
        AssignOpKind::RemAssign => "assign:%=",
        AssignOpKind::BitXorAssign => "assign:^=",
        AssignOpKind::BitAndAssign => "assign:&=",
        AssignOpKind::BitOrAssign => "assign:|=",
        AssignOpKind::ShlAssign => "assign:<<=",
        AssignOpKind::ShrAssign => "assign:>>=",
    }
}
