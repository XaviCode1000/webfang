//! AST-based extractor of intra-crate layer references and port-choke constructs.
//!
//! The tool is an EXTRACTOR, not a gate: it reports every site it finds and
//! leaves every policy decision (which layers count, which direction is legal,
//! which sites are allowlisted) to the shell that consumes its output.
//!
//! What it guarantees:
//!
//! * **Structural test regions (B36).** A node is inside a test region when it
//!   carries a `#[cfg(P)]` whose predicate positively mentions `test`. The
//!   exclusion is the test-module SUBTREE, so code that follows a
//!   `#[cfg(test)] mod` in the same file is still scanned.
//! * **No comment or string blindness (B37).** `syn` drops comments and turns
//!   strings into literal values, so a path inside a string literal is
//!   invisible and a `/*` inside a string cannot hide anything.
//! * **Every path form (B38).** Use trees, renamed imports, globs, nested
//!   groups and inline qualified paths are all reported, normalised.
//! * **Port-choke constructs (B39).** Raw client wiring (`dns_resolver`,
//!   `redirect`) and any mention of the concrete `ValidatingResolver`.
//!
//! The CLI contract lives in `main.rs`; this module is the reusable core.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use proc_macro2::{Spacing, TokenTree};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::Path as SynPath;
use syn::{Attribute, Expr, Item, ItemUse, Meta, Token, UseTree};

/// The path prefix that marks an intra-crate reference.
pub const CRATE_ROOT: &str = "crate";

/// The concrete resolver whose mention bypasses the SSRF guard choke point.
pub const VALIDATING_RESOLVER: &str = "ValidatingResolver";

/// Method calls that wire a raw HTTP client, bypassing the port choke point.
pub const CHOKE_METHODS: [&str; 2] = ["dns_resolver", "redirect"];

/// The kind of a reported site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A leaf of a `use` / `pub use` statement.
    Use,
    /// A qualified path in expression, type or pattern position.
    Inline,
    /// A port-choke construct (raw client wiring or a concrete resolver).
    Choke,
}

impl Kind {
    /// The wire representation of the kind, as used in the TSV output.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Use => "use",
            Kind::Inline => "inline",
            Kind::Choke => "choke",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One reported site: `<relative-file>\t<line>\t<kind>\t<path>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    /// Path of the file, relative to the scanned root.
    pub file: String,
    /// 1-indexed line of the construct.
    pub line: usize,
    /// The kind of the site.
    pub kind: Kind,
    /// The normalised path (no leading `::`, no spaces).
    pub path: String,
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}",
            self.file, self.line, self.kind, self.path
        )
    }
}

/// A file that could not be read or parsed.
#[derive(Debug)]
pub enum ExtractError {
    /// The file could not be read.
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },
    /// The file could not be parsed as Rust source.
    Parse {
        /// The file that could not be parsed.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::Read { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            },
            ExtractError::Parse { path, message } => {
                write!(f, "cannot parse {}: {message}", path.display())
            },
        }
    }
}

impl Error for ExtractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExtractError::Read { source, .. } => Some(source),
            ExtractError::Parse { .. } => None,
        }
    }
}

/// The outcome of scanning a whole tree.
#[derive(Debug, Default)]
pub struct TreeExtraction {
    /// Every site found in the files that could be read and parsed.
    pub sites: Vec<Site>,
    /// One entry per file that failed; a failure is never a silent skip.
    pub errors: Vec<ExtractError>,
}

/// Extract every site from one file.
///
/// `file` is reported relative to `root`. A file that cannot be read or parsed
/// is an error, never an empty result.
pub fn extract_file(root: &Path, file: &Path) -> Result<Vec<Site>, ExtractError> {
    let source = fs::read_to_string(file).map_err(|source| ExtractError::Read {
        path: file.to_path_buf(),
        source,
    })?;
    let parsed = syn::parse_file(&source).map_err(|error| ExtractError::Parse {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut extractor = Extractor {
        file: relative_path(root, file),
        sites: Vec::new(),
    };
    extractor.visit_file(&parsed);
    Ok(extractor.sites)
}

/// Extract every site from every `.rs` file under `root`.
///
/// The returned sites are sorted and free of exact duplicates. Files that fail
/// to parse are reported in `errors`; the scan continues so one broken file
/// cannot hide the rest of the tree.
pub fn extract_tree(root: &Path) -> Result<TreeExtraction, ExtractError> {
    let files = collect_rs_files(root)?;
    let mut extraction = TreeExtraction::default();
    for file in files {
        match extract_file(root, &file) {
            Ok(sites) => extraction.sites.extend(sites),
            Err(error) => extraction.errors.push(error),
        }
    }
    extraction.sites.sort();
    extraction.sites.dedup();
    Ok(extraction)
}

/// Every `.rs` file under `root`, in a deterministic order.
fn collect_rs_files(root: &Path) -> Result<Vec<PathBuf>, ExtractError> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| ExtractError::Read {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ExtractError::Read {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| ExtractError::Read {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() && is_rust_file(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn is_rust_file(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension == "rs")
        .unwrap_or(false)
}

/// The path of `file` relative to `root`, falling back to the file name.
fn relative_path(root: &Path, file: &Path) -> String {
    match file.strip_prefix(root) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.to_string_lossy().into_owned()),
    }
}

/// Collects the sites of one file.
struct Extractor {
    file: String,
    sites: Vec<Site>,
}

impl Extractor {
    fn push(&mut self, line: usize, kind: Kind, path: String) {
        self.sites.push(Site {
            file: self.file.clone(),
            line,
            kind,
            path,
        });
    }

    /// Record a path, plus its choke construct.
    ///
    /// The path itself is only reported when its first segment is `crate`; the
    /// choke construct is reported for every path, because a mention of the
    /// concrete resolver bypasses the port whatever module it lives in.
    ///
    /// `suffix` is appended to the rendered path when the path is the receiver
    /// of a method call (`::<method>`), so the record identifies the whole
    /// construct instead of the bare path.
    fn record_path(&mut self, path: &SynPath, suffix: Option<&str>) {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let line = path_line(path);
        self.push_resolver_choke(&segments, line);
        if !is_crate_rooted(path) {
            return;
        }
        let mut rendered = segments.join("::");
        if let Some(suffix) = suffix {
            rendered.push_str("::");
            rendered.push_str(suffix);
        }
        self.push(line, Kind::Inline, rendered);
    }

    /// Record the port-choke construct of a path, if it names one.
    ///
    /// A path that ends with `ValidatingResolver` (a bare type mention) or with
    /// `ValidatingResolver::<segment>` (`::new`, `::with_policy`,
    /// `::with_allow_loopback`) bypasses the choke point.
    fn push_resolver_choke(&mut self, segments: &[String], line: usize) {
        for (index, segment) in segments.iter().enumerate() {
            if segment != VALIDATING_RESOLVER {
                continue;
            }
            let reported = match segments.get(index + 1) {
                Some(next) => format!("{VALIDATING_RESOLVER}::{next}"),
                None => VALIDATING_RESOLVER.to_string(),
            };
            self.push(line, Kind::Choke, reported);
            return;
        }
    }
}

impl<'ast> Visit<'ast> for Extractor {
    /// Skip every item that sits in a test region.
    ///
    /// The exclusion is the item's SUBTREE, so a `#[cfg(test)] mod` hides only
    /// its own contents — code that follows it in the same file is scanned.
    fn visit_item(&mut self, item: &'ast Item) {
        if is_test_region(item_attributes(item)) {
            return;
        }
        visit::visit_item(self, item);
    }

    /// Skip an associated item that sits in a test region.
    ///
    /// A `#[cfg(test)]` method inside an `impl` block is a test region of its
    /// own, even though the block itself is production code.
    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        if is_test_region(impl_item_attributes(item)) {
            return;
        }
        visit::visit_impl_item(self, item);
    }

    /// Skip a trait item that sits in a test region.
    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        if is_test_region(trait_item_attributes(item)) {
            return;
        }
        visit::visit_trait_item(self, item);
    }

    /// Skip an enum variant that sits in a test region.
    fn visit_variant(&mut self, variant: &'ast syn::Variant) {
        if is_test_region(&variant.attrs) {
            return;
        }
        visit::visit_variant(self, variant);
    }

    /// Skip a struct field that sits in a test region.
    fn visit_field(&mut self, field: &'ast syn::Field) {
        if is_test_region(&field.attrs) {
            return;
        }
        visit::visit_field(self, field);
    }

    /// Report every leaf of a use statement, on the statement's line.
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        let line = line_of(node.use_token.span);
        let mut leaves = Vec::new();
        flatten_use_tree(&node.tree, "", &mut leaves);
        for leaf in leaves {
            if !starts_with_crate(&leaf) {
                continue;
            }
            self.push(line, Kind::Use, leaf.clone());
            let segments: Vec<String> = leaf.split("::").map(str::to_string).collect();
            self.push_resolver_choke(&segments, line);
        }
    }

    /// Report qualified paths in expression, type and pattern position.
    fn visit_path(&mut self, path: &'ast SynPath) {
        self.record_path(path, None);
        visit::visit_path(self, path);
    }

    /// Skip the path of a restricted visibility.
    ///
    /// `pub(crate)` carries a `crate` path that restricts the visibility, not
    /// a reference to a module, so it is not a site.
    fn visit_visibility(&mut self, _visibility: &'ast syn::Visibility) {}

    /// Report method calls: the receiver path gains the method name, and the
    /// raw client wiring methods are port-choke constructs.
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if CHOKE_METHODS.contains(&method.as_str()) {
            self.push(line_of(node.method.span()), Kind::Choke, method.clone());
        }
        match &*node.receiver {
            Expr::Path(path) => self.record_path(&path.path, Some(&method)),
            receiver => self.visit_expr(receiver),
        }
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        for argument in &node.args {
            self.visit_expr(argument);
        }
        if let Some(turbofish) = &node.turbofish {
            self.visit_angle_bracketed_generic_arguments(turbofish);
        }
    }

    /// Report the macro's own path and every crate-rooted path in its body.
    ///
    /// `syn` does not parse macro bodies, so the token stream is walked
    /// directly: an identifier `crate` followed by `::` and another identifier
    /// starts a path that is reconstructed from the tokens.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.visit_path(&node.path);
        let tokens: Vec<TokenTree> = node.tokens.clone().into_iter().collect();
        self.scan_tokens(&tokens);
    }
}

impl Extractor {
    /// Walk a token stream looking for crate-rooted paths.
    fn scan_tokens(&mut self, tokens: &[TokenTree]) {
        let mut index = 0;
        while index < tokens.len() {
            match &tokens[index] {
                TokenTree::Ident(ident) if ident == CRATE_ROOT => {
                    match parse_macro_path(tokens, index) {
                        Some((path, next, path_line)) => {
                            self.push(path_line, Kind::Inline, path);
                            index = next;
                        },
                        None => index += 1,
                    }
                },
                TokenTree::Group(group) => {
                    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                    self.scan_tokens(&inner);
                    index += 1;
                },
                _ => index += 1,
            }
        }
    }
}

/// Reconstruct a crate-rooted path from a token stream.
///
/// Returns the normalised path, the index of the first token after it and the
/// line the path starts on.
fn parse_macro_path(tokens: &[TokenTree], start: usize) -> Option<(String, usize, usize)> {
    let mut segments = vec![CRATE_ROOT.to_string()];
    let mut index = start + 1;
    // The separator is two tokens (':' ':'), so the segment follows it.
    while index + 2 < tokens.len() {
        if !is_path_separator(&tokens[index], &tokens[index + 1]) {
            break;
        }
        let TokenTree::Ident(ident) = &tokens[index + 2] else {
            break;
        };
        if !is_path_segment(ident) {
            break;
        }
        segments.push(ident.to_string());
        index += 3;
    }
    if segments.len() == 1 {
        return None;
    }
    let line = match &tokens[start] {
        TokenTree::Ident(ident) => line_of(ident.span()),
        _ => 1,
    };
    Some((segments.join("::"), index, line))
}

/// Whether two tokens form the `::` separator.
fn is_path_separator(first: &TokenTree, second: &TokenTree) -> bool {
    match (first, second) {
        (TokenTree::Punct(left), TokenTree::Punct(right)) => {
            left.as_char() == ':' && right.as_char() == ':' && left.spacing() == Spacing::Joint
        },
        _ => false,
    }
}

/// Whether an identifier can be a path segment.
fn is_path_segment(ident: &proc_macro2::Ident) -> bool {
    let text = ident.to_string();
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if unicode_ident::is_xid_start(first) => {},
        _ => return false,
    }
    characters.all(unicode_ident::is_xid_continue)
}

/// Flatten a use tree into its leaf paths, concatenated from the tree root.
fn flatten_use_tree(tree: &UseTree, prefix: &str, leaves: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => {
            let next = join_segment(prefix, &path.ident.to_string());
            match &*path.tree {
                // `use a::b;` parses as Path(a) -> Path(b) -> Name(b): the
                // trailing name repeats the last segment instead of adding one.
                UseTree::Name(name) if name.ident == path.ident => leaves.push(next),
                inner => flatten_use_tree(inner, &next, leaves),
            }
        },
        UseTree::Name(name) => {
            if name.ident == "self" {
                if !prefix.is_empty() {
                    leaves.push(prefix.to_string());
                }
            } else {
                leaves.push(join_segment(prefix, &name.ident.to_string()));
            }
        },
        // The alias is a local binding, not part of the referenced path.
        UseTree::Rename(rename) => {
            if rename.ident == "self" {
                // `use a::b::{self as c}` imports the module `a::b` itself.
                if !prefix.is_empty() {
                    leaves.push(prefix.to_string());
                }
            } else {
                leaves.push(join_segment(prefix, &rename.ident.to_string()));
            }
        },
        // A glob denotes the module itself.
        UseTree::Glob(_) => {
            if !prefix.is_empty() {
                leaves.push(prefix.to_string());
            }
        },
        UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(item, prefix, leaves);
            }
        },
    }
}

fn join_segment(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}::{segment}")
    }
}

/// Whether a path starts with the `crate` segment.
fn is_crate_rooted(path: &SynPath) -> bool {
    path.segments
        .first()
        .map(|segment| segment.ident == CRATE_ROOT)
        .unwrap_or(false)
}

/// Whether a flattened path starts with the `crate` segment.
fn starts_with_crate(path: &str) -> bool {
    path.split("::").next() == Some(CRATE_ROOT)
}

/// The line of a path, taken from its first segment.
fn path_line(path: &SynPath) -> usize {
    path.segments
        .first()
        .map(|segment| line_of(segment.ident.span()))
        .unwrap_or(1)
}

/// The 1-indexed line a span points at.
fn line_of(span: proc_macro2::Span) -> usize {
    span.start().line
}

/// The attributes of an item.
///
/// `syn` has no `Item::attrs` accessor, so the variants are matched by hand.
fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

/// The attributes of an associated item.
fn impl_item_attributes(item: &syn::ImplItem) -> &[Attribute] {
    match item {
        syn::ImplItem::Const(item) => &item.attrs,
        syn::ImplItem::Fn(item) => &item.attrs,
        syn::ImplItem::Type(item) => &item.attrs,
        syn::ImplItem::Macro(item) => &item.attrs,
        _ => &[],
    }
}

/// The attributes of a trait item.
fn trait_item_attributes(item: &syn::TraitItem) -> &[Attribute] {
    match item {
        syn::TraitItem::Const(item) => &item.attrs,
        syn::TraitItem::Fn(item) => &item.attrs,
        syn::TraitItem::Type(item) => &item.attrs,
        syn::TraitItem::Macro(item) => &item.attrs,
        _ => &[],
    }
}

/// Whether an item sits in a test region.
///
/// A test region is an item carrying a `#[cfg(P)]` whose predicate positively
/// mentions `test`: `cfg(test)`, `cfg(all(test, ...))` and `cfg(any(test, ...))`
/// qualify, while `cfg(not(test))`, `cfg(feature = "...")` and `cfg_attr(...)`
/// do not.
fn is_test_region(attributes: &[Attribute]) -> bool {
    attributes
        .iter()
        .any(|attribute| attribute.path().is_ident("cfg") && is_test_cfg(attribute))
}

/// Whether a `#[cfg(...)]` attribute names a test region.
fn is_test_cfg(attribute: &Attribute) -> bool {
    let Meta::List(list) = &attribute.meta else {
        return false;
    };
    match list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
        Ok(predicates) => predicates.iter().any(meta_mentions_test),
        Err(_) => false,
    }
}

/// Whether a cfg predicate positively mentions `test`.
///
/// `not(...)` never qualifies: it negates its operand, so `cfg(not(test))` is
/// not a test region.
fn meta_mentions_test(meta: &Meta) -> bool {
    match meta {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => {
            if !list.path.is_ident("all") && !list.path.is_ident("any") {
                return false;
            }
            match list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
                Ok(nested) => nested.iter().any(meta_mentions_test),
                Err(_) => false,
            }
        },
        Meta::NameValue(_) => false,
    }
}
