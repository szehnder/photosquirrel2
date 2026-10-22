//! Enforces ADR-0003: core crates must not (transitively) depend on tokio or gpui-kit;
//! I/O crates must not depend on gpui-kit. Also enforces ADR-0004 for the scanner: `ps-scan`'s sources
//! contain no call that could create, write, rename, remove or re-permission a file. ADR-0004/0024/0061 for the
//! whole workspace: renaming, linking, copying or removing files is allowed only where [`MOVE_ALLOWED`] lists it
//! (user files only in `ps-export`'s copy protocol and quarantine module; elsewhere only PhotoSquirrel's own
//! data files). The move check parses each file (`syn`) and scans its tokens, so comments and string literals
//! are not code, imports (`use std::fs::{rename as mv}`) are followed and `#[cfg(test)]` items are skipped
//! (ADR-0062). And
//! ADR-0027: UI crates reach the engine only through the `ps-service` facade, never by a direct dependency on
//! an engine crate.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

const CORE: &[&str] = &["ps-model", "ps-media", "ps-hash", "ps-dedup", "ps-fixtures"];
const IO: &[&str] = &[
    "ps-catalog",
    "ps-scan",
    "ps-llm",
    "ps-export",
    "ps-service",
    "photosquirrel-cli",
];
const UI: &[&str] = &["ui-theme", "ui-gallery", "ui-workspace", "photosquirrel"];
/// Engine crates UI crates must not depend on directly (dev-dependencies for tests are fine).
const BEHIND_FACADE: &[&str] = &["ps-catalog", "ps-scan", "ps-llm", "ps-export"];

/// Direct dependencies of a UI crate that bypass the facade, as violation messages.
pub fn facade_violations(krate: &str, direct: &[&str]) -> Vec<String> {
    direct
        .iter()
        .filter(|d| BEHIND_FACADE.contains(d))
        .map(|d| format!("UI crate `{krate}` must reach `{d}` through ps-service (ADR-0027)"))
        .collect()
}

pub fn check(root: &Path) -> Result<()> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let meta: Value = serde_json::from_slice(&out.stdout)?;

    // package id -> name, and id -> normal (non-dev, non-build) dependency ids
    let mut names: HashMap<String, String> = HashMap::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        names.insert(
            p["id"].as_str().unwrap_or_default().into(),
            p["name"].as_str().unwrap_or_default().into(),
        );
    }
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for node in meta["resolve"]["nodes"].as_array().into_iter().flatten() {
        let id = node["id"].as_str().unwrap_or_default().to_string();
        let normal = node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k["kind"].is_null())
            })
            .filter_map(|d| d["pkg"].as_str().map(String::from))
            .collect();
        deps.insert(id, normal);
    }

    let members: HashMap<String, String> = meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str())
        .map(|id| (names[id].clone(), id.to_string()))
        .collect();

    let mut violations = read_only_violations(&root.join("crates/ps-scan/src"))?;
    violations.extend(move_violations(root)?);
    for (name, id) in &members {
        if UI.contains(&name.as_str()) {
            let direct: Vec<&str> = deps
                .get(id)
                .into_iter()
                .flatten()
                .filter_map(|d| names.get(d).map(String::as_str))
                .collect();
            violations.extend(facade_violations(name, &direct));
        }
        let forbidden: &[&str] = if CORE.contains(&name.as_str()) {
            &["tokio", "gpui-kit"]
        } else if IO.contains(&name.as_str()) {
            &["gpui-kit"]
        } else if UI.contains(&name.as_str()) || name == "xtask" {
            &[]
        } else {
            violations.push(format!(
                "crate `{name}` is not classified in xtask/src/layering.rs"
            ));
            continue;
        };
        if forbidden.is_empty() {
            continue;
        }
        let reachable = closure(id, &deps);
        for f in forbidden {
            if reachable
                .iter()
                .any(|d| names.get(d).is_some_and(|n| n == f))
            {
                violations.push(format!("`{name}` must not depend on `{f}` (ADR-0003)"));
            }
        }
    }
    if violations.is_empty() {
        println!("layering OK ({} crates checked)", members.len());
        Ok(())
    } else {
        for v in &violations {
            eprintln!("  {v}");
        }
        bail!("{} layering violation(s)", violations.len())
    }
}

fn closure(start: &str, deps: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(id) = stack.pop() {
        for d in deps.get(&id).into_iter().flatten() {
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    seen
}

/// Calls `ps-scan` must never make (ADR-0004: the scanner opens source files read-only).
pub const SCAN_FORBIDDEN: &[&str] = &[
    "OpenOptions::write",
    ".write(true)",
    ".append(true)",
    ".create(true)",
    ".create_new(true)",
    ".truncate(true)",
    "File::create",
    "File::options",
    "fs::remove_",
    "remove_file(",
    "remove_dir",
    "fs::rename",
    "set_permissions",
    "fs::write",
    "fs::copy",
    "fs::hard_link",
    "set_len(",
    "set_modified(",
];

/// Forbidden calls in one source file, as `file:line: call` strings. Matched on the file's code tokens per
/// line (comments and string literals are not code; a file that does not lex is a violation).
pub fn forbidden_calls(file: &str, text: &str) -> Vec<String> {
    let lines = match code_by_line(text) {
        Ok(l) => l,
        Err(e) => {
            return vec![format!(
                "{file}: cannot be lexed for the read-only check: {e}"
            )];
        }
    };
    let mut out = Vec::new();
    for (line, code) in lines {
        for f in SCAN_FORBIDDEN {
            if code.contains(f) {
                out.push(format!(
                    "{file}:{line}: `{f}` is forbidden in ps-scan (ADR-0004: sources are read-only)"
                ));
            }
        }
    }
    out
}

/// The code tokens of each line, joined without spaces (string and other literals become `_`).
fn code_by_line(text: &str) -> Result<Vec<(usize, String)>, String> {
    use proc_macro2::{Delimiter, TokenStream, TokenTree};
    fn walk(ts: TokenStream, out: &mut std::collections::BTreeMap<usize, String>) {
        for t in ts {
            let line = t.span().start().line;
            match t {
                TokenTree::Group(g) => {
                    let (open, close) = match g.delimiter() {
                        Delimiter::Parenthesis => ("(", ")"),
                        Delimiter::Brace => ("{", "}"),
                        Delimiter::Bracket => ("[", "]"),
                        Delimiter::None => ("", ""),
                    };
                    out.entry(line).or_default().push_str(open);
                    walk(g.stream(), out);
                    let end = g.span_close().start().line;
                    out.entry(end).or_default().push_str(close);
                }
                TokenTree::Ident(i) => out.entry(line).or_default().push_str(&i.to_string()),
                TokenTree::Punct(p) => out.entry(line).or_default().push(p.as_char()),
                TokenTree::Literal(_) => out.entry(line).or_default().push('_'),
            }
        }
    }
    let ts: TokenStream = text
        .parse()
        .map_err(|e: proc_macro2::LexError| e.to_string())?;
    let mut out = std::collections::BTreeMap::new();
    walk(ts, &mut out);
    Ok(out.into_iter().collect())
}

fn read_only_violations(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p)?;
                out.extend(forbidden_calls(&p.display().to_string(), &text));
            }
        }
    }
    Ok(out)
}

/// Kinds of call that rename, link, copy or remove files (ADR-0004: no code path deletes a user file;
/// ADR-0024/0061/0062: only `ps-export::quarantine` moves user files, only by a no-replace move). Each is a
/// set of paths ([`move_kind`]); [`MOVE_ALLOWED`] names the kinds a file may use.
pub const MOVE_CALLS: &[&str] = &[
    "fs::rename",
    "remove_file",
    "unlink",
    "remove_dir",
    "remove_dir_all",
    "renameat",
    "hard_link",
    "fs::copy",
    "MoveFileEx",
    "rename_no_replace",
    "link_unlink",
    "move_no_replace",
    "link_then_unlink",
    "shell_command",
    "persist",
];

/// Programs that delete or move files; spawning one ([`find_move_calls`] reads `Command::new("rm")`, also with
/// a path or a `.exe`) is the `shell_command` kind, allowed nowhere.
pub const SHELL_FILE_COMMANDS: &[&str] = &[
    "rm", "rmdir", "del", "erase", "rd", "mv", "move", "unlink", "shred", "trash",
];

/// The kind ([`MOVE_CALLS`]) of a fully resolved path, if it renames, links, copies or removes files:
/// `std::fs` / `tokio::fs` / `fs_err` functions, their `libc`, `rustix` and `nix` equivalents, the
/// `remove_dir_all` crate, `tempfile`'s `persist` (it replaces its target; `persist_noclobber` does not), the
/// Windows move and delete functions, and the workspace's own move primitives (by name, wherever they are
/// called from).
pub fn move_kind(path: &[String]) -> Option<&'static str> {
    let last = path.last()?.as_str();
    let own = [
        "rename_no_replace",
        "link_unlink",
        "move_no_replace",
        "link_then_unlink",
    ];
    if let Some(k) = own.iter().find(|k| **k == last) {
        return Some(k);
    }
    // Win32 (`windows-sys` / `windows` / hand-written FFI), by exact name.
    let win32 = [
        "MoveFileA",
        "MoveFileW",
        "MoveFileExA",
        "MoveFileExW",
        "MoveFileWithProgressA",
        "MoveFileWithProgressW",
        "MoveFileTransactedW",
        "ReplaceFileA",
        "ReplaceFileW",
        "DeleteFileA",
        "DeleteFileW",
        "RemoveDirectoryA",
        "RemoveDirectoryW",
        "CreateHardLinkA",
        "CreateHardLinkW",
        "CopyFileA",
        "CopyFileW",
        "CopyFileExW",
        "CopyFile2",
        "SetFileInformationByHandle",
    ];
    if win32.contains(&last) {
        return Some("MoveFileEx");
    }
    let parent: Vec<&str> = path[..path.len() - 1].iter().map(String::as_str).collect();
    let std_like = matches!(
        parent.as_slice(),
        ["std", "fs"] | ["tokio", "fs"] | ["async_std", "fs"] | ["fs_err"] | ["fs_err", "tokio"]
    );
    // The `remove_dir_all` crate: every function removes a tree.
    if parent.first() == Some(&"remove_dir_all") {
        return Some("remove_dir_all");
    }
    // `NamedTempFile::persist` / `TempPath::persist` replace an existing target.
    if last == "persist" && matches!(parent.last(), Some(&"NamedTempFile") | Some(&"TempPath")) {
        return Some("persist");
    }
    let sys = matches!(
        parent.as_slice(),
        ["libc"] | ["rustix", "fs"] | ["nix", "unistd"] | ["nix", "fcntl"]
    );
    match last {
        "rename" if std_like || sys => Some("fs::rename"),
        "remove_file" if std_like => Some("remove_file"),
        "remove" if sys => Some("remove_file"),
        "unlink" | "unlinkat" if sys => Some("unlink"),
        "remove_dir" if std_like => Some("remove_dir"),
        "rmdir" if sys => Some("remove_dir"),
        "remove_dir_all" if std_like => Some("remove_dir_all"),
        "renameat" | "renameat2" | "renameat_with" | "renameatx_np" if sys => Some("renameat"),
        "hard_link" if std_like => Some("hard_link"),
        "link" | "linkat" if sys => Some("hard_link"),
        "copy" if std_like => Some("fs::copy"),
        "copy_file_range" | "copyfile" | "fcopyfile" | "sendfile" if sys => Some("fs::copy"),
        _ => None,
    }
}

/// Where [`MOVE_CALLS`] may appear: (source file or folder prefix, calls allowed there, why). Everything else
/// in `crates/*/{src,benches,examples}`, `crates/*/build.rs` and `xtask/` is a violation. `#[cfg(test)]` items
/// and integration tests (`crates/*/tests`) use temp folders and are not checked.
pub const MOVE_ALLOWED: &[(&str, &[&str], &str)] = &[
    (
        "crates/ps-export/src/rename.rs",
        &[
            "fs::rename",
            "remove_file",
            "renameat",
            "rename_no_replace",
            "link_unlink",
            "hard_link",
        ],
        "the export no-replace rename primitive (ADR-0038), library partials only",
    ),
    (
        "crates/ps-export/src/copy.rs",
        &["rename_no_replace"],
        "renames a verified `.ps-partial` into the library (ADR-0022)",
    ),
    (
        "crates/ps-export/src/library.rs",
        &["remove_file"],
        "removes the exporter's own `.ps-partial` files inside the library",
    ),
    (
        "crates/ps-export/src/quarantine/fsops.rs",
        &[
            "renameat",
            "MoveFileEx",
            "hard_link",
            "remove_file",
            "link_then_unlink",
            "remove_dir",
            "move_no_replace",
        ],
        "the only move of user files: same-volume, no-replace, journaled (ADR-0061, ADR-0062); `remove_file` \
         only as the unlink of a link+unlink move, after the link succeeded and both names are the same file; \
         `remove_dir` only of empty folders inside the run's quarantine folder",
    ),
    (
        "crates/ps-export/src/quarantine/",
        &["move_no_replace"],
        "cleanup and undo call the quarantine move primitive",
    ),
    (
        "crates/photosquirrel-cli/src/control.rs",
        &["fs::rename", "remove_file"],
        "the CLI's own control and heartbeat files in the data folder",
    ),
    (
        "crates/ui-workspace/src/persist.rs",
        &["fs::rename"],
        "atomic replace of the app's own window-state file",
    ),
    (
        "crates/ps-service/src/json_store.rs",
        &["fs::rename"],
        "atomic replace of the app's own JSON stores in the data folder",
    ),
    (
        "crates/ps-service/src/settings.rs",
        &["fs::rename", "remove_file"],
        "atomic replace of settings.toml and its temp file",
    ),
    (
        "crates/ps-service/src/library/synthetic.rs",
        &["remove_file"],
        "the synthetic benchmark catalog's own files in its marked folder",
    ),
    (
        "crates/ps-media/src/preview_cache.rs",
        &["fs::rename", "remove_file"],
        "the preview cache's own files in the data folder",
    ),
    (
        "crates/ps-fixtures/src/shared.rs",
        &["remove_dir", "remove_dir_all", "remove_file"],
        "test fixture folders under the cargo target dir",
    ),
    (
        "xtask/src/screenshot.rs",
        &["remove_dir_all"],
        "the screenshot tool's own scratch data folders under the target dir",
    ),
    (
        "xtask/src/bench.rs",
        &["remove_dir_all", "remove_file"],
        "the benchmark's own scratch catalogs and result files under the target dir",
    ),
    (
        "xtask/src/package.rs",
        &["remove_dir_all", "remove_file", "fs::copy"],
        "staging release packages from build outputs under the target dir",
    ),
];

/// One move call found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveCall {
    pub line: usize,
    pub kind: &'static str,
    /// The resolved path (`std::fs::rename`).
    pub path: String,
}

/// What [`move_calls`] needs from the parsed file: imported names and the lines of `#[cfg(test)]` items.
#[derive(Default)]
struct FileFacts {
    /// Local name -> the path it stands for (`rename` -> `std::fs::rename`, `fs` -> `std::fs`).
    aliases: HashMap<String, Vec<String>>,
    /// Glob imports (`use std::fs::*`): their paths.
    globs: Vec<Vec<String>>,
    /// Line ranges of items under exactly `#[cfg(test)]`.
    test_lines: Vec<(usize, usize)>,
}

impl FileFacts {
    fn use_tree(&mut self, prefix: &mut Vec<String>, t: &syn::UseTree) {
        match t {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.use_tree(prefix, &p.tree);
                prefix.pop();
            }
            syn::UseTree::Name(n) => {
                let name = n.ident.to_string();
                if name == "self" {
                    if let Some(last) = prefix.last() {
                        self.aliases.insert(last.clone(), prefix.clone());
                    }
                } else {
                    let mut full = prefix.clone();
                    full.push(name.clone());
                    self.aliases.insert(name, full);
                }
            }
            syn::UseTree::Rename(r) => {
                let mut full = prefix.clone();
                if r.ident != "self" {
                    full.push(r.ident.to_string());
                }
                self.aliases.insert(r.rename.to_string(), full);
            }
            syn::UseTree::Glob(_) => self.globs.push(prefix.clone()),
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    self.use_tree(prefix, t);
                }
            }
        }
    }

    /// Exactly `#[cfg(test)]`: code compiled only into unit tests. `cfg(any(test, feature = ".."))` is also
    /// compiled with the feature, and `cfg(not(test))` everywhere but tests, so both are checked.
    fn cfg_test(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            let syn::Meta::List(l) = &a.meta else {
                return false;
            };
            a.path().is_ident("cfg") && l.tokens.to_string().trim() == "test"
        })
    }

    fn skip_if_test(&mut self, attrs: &[syn::Attribute], span: proc_macro2::Span) {
        if Self::cfg_test(attrs) {
            self.test_lines.push((span.start().line, span.end().line));
        }
    }

    fn is_test_line(&self, line: usize) -> bool {
        self.test_lines
            .iter()
            .any(|(a, b)| (*a..=*b).contains(&line))
    }
}

impl<'ast> syn::visit::Visit<'ast> for FileFacts {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let mut prefix = Vec::new();
        self.use_tree(&mut prefix, &u.tree);
    }
    fn visit_item(&mut self, i: &'ast syn::Item) {
        use syn::spanned::Spanned;
        let attrs: &[syn::Attribute] = match i {
            syn::Item::Mod(x) => &x.attrs,
            syn::Item::Fn(x) => &x.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Use(x) => &x.attrs,
            syn::Item::Const(x) => &x.attrs,
            syn::Item::Static(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            syn::Item::Macro(x) => &x.attrs,
            _ => &[],
        };
        if Self::cfg_test(attrs) {
            self.skip_if_test(attrs, i.span());
            return;
        }
        syn::visit::visit_item(self, i);
    }
    fn visit_stmt(&mut self, s: &'ast syn::Stmt) {
        use syn::spanned::Spanned;
        if let syn::Stmt::Local(l) = s
            && Self::cfg_test(&l.attrs)
        {
            self.skip_if_test(&l.attrs, s.span());
            return;
        }
        syn::visit::visit_stmt(self, s);
    }
}

/// Every move call in a file's code: each path in its tokens (macro bodies included), resolved through the
/// file's imports, whose [`move_kind`] is set. Outside `#[cfg(test)]` items; comments and literals are not
/// code. Errors: the file does not parse.
pub fn find_move_calls(text: &str) -> Result<Vec<MoveCall>, String> {
    use proc_macro2::{Spacing, TokenStream, TokenTree};
    // A snippet of statements (unit tests) parses as the body of a function, one line lower.
    let (parsed, shift) = match syn::parse_file(text) {
        Ok(f) => (f, 0),
        Err(e) => match syn::parse_file(&format!("fn __xtask_snippet() {{\n{text}\n}}")) {
            Ok(f) => (f, 1),
            Err(_) => return Err(e.to_string()),
        },
    };
    let mut facts = FileFacts::default();
    syn::visit::Visit::visit_file(&mut facts, &parsed);
    for r in &mut facts.test_lines {
        *r = (r.0.saturating_sub(shift), r.1.saturating_sub(shift));
    }

    #[derive(Debug)]
    enum Tok {
        Ident(String, usize),
        PathSep,
        Dot,
        /// The start of a `( ... )` group.
        OpenParen,
        /// A string literal's value.
        Str(String),
        Other,
    }
    /// The value of a (raw) string literal token, without escapes processed.
    fn str_value(lit: &proc_macro2::Literal) -> Option<String> {
        let t = lit.to_string();
        let t = t
            .strip_prefix('r')
            .map(|r| r.trim_matches('#'))
            .unwrap_or(&t);
        t.strip_prefix('"')?.strip_suffix('"').map(str::to_owned)
    }
    fn flatten(ts: TokenStream, out: &mut Vec<Tok>) {
        let mut it = ts.into_iter().peekable();
        while let Some(t) = it.next() {
            match t {
                TokenTree::Ident(i) => {
                    let s = i.to_string();
                    let s = s.strip_prefix("r#").map(str::to_owned).unwrap_or(s);
                    out.push(Tok::Ident(s, i.span().start().line));
                }
                TokenTree::Punct(p) if p.as_char() == ':' && p.spacing() == Spacing::Joint => {
                    if matches!(it.peek(), Some(TokenTree::Punct(q)) if q.as_char() == ':') {
                        it.next();
                        out.push(Tok::PathSep);
                    } else {
                        out.push(Tok::Other);
                    }
                }
                TokenTree::Punct(p) if p.as_char() == '.' => out.push(Tok::Dot),
                TokenTree::Group(g) => {
                    out.push(if g.delimiter() == proc_macro2::Delimiter::Parenthesis {
                        Tok::OpenParen
                    } else {
                        Tok::Other
                    });
                    flatten(g.stream(), out);
                    out.push(Tok::Other);
                }
                TokenTree::Literal(l) => out.push(str_value(&l).map_or(Tok::Other, Tok::Str)),
                _ => out.push(Tok::Other),
            }
        }
    }
    let ts: TokenStream = text
        .parse()
        .map_err(|e: proc_macro2::LexError| e.to_string())?;
    let mut toks = Vec::new();
    flatten(ts, &mut toks);

    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Ident(first, line) = &toks[i] else {
            i += 1;
            continue;
        };
        // A method or field (`x.rename(...)`) is not a path; neither is the tail of a longer path.
        let after_dot = i > 0 && matches!(toks[i - 1], Tok::Dot);
        let mut segs = vec![first.clone()];
        let mut j = i + 1;
        while j + 1 < toks.len()
            && matches!(toks[j], Tok::PathSep)
            && let Tok::Ident(s, _) = &toks[j + 1]
        {
            segs.push(s.clone());
            j += 2;
        }
        let line = *line;
        i = j;
        if facts.is_test_line(line) {
            continue;
        }
        let called = matches!(toks.get(j), Some(Tok::OpenParen));
        if after_dot {
            // `tmp.persist(path)`: `tempfile`'s persist replaces an existing target (`persist_noclobber`, a
            // different name, does not). No other type in the workspace has a `persist` method.
            if segs.len() == 1 && segs[0] == "persist" && called {
                out.push(MoveCall {
                    line,
                    kind: "persist",
                    path: ".persist".into(),
                });
            }
            continue;
        }
        let mut full: Vec<String> = match facts.aliases.get(&segs[0]) {
            Some(p) => p.iter().cloned().chain(segs[1..].iter().cloned()).collect(),
            None => segs.clone(),
        };
        if full.len() == 1
            && let Some(g) = facts.globs.iter().find(|g| {
                let mut p = (*g).clone();
                p.push(full[0].clone());
                move_kind(&p).is_some()
            })
        {
            let mut p = g.clone();
            p.push(full.remove(0));
            full = p;
        }
        if let Some(kind) = move_kind(&full) {
            out.push(MoveCall {
                line,
                kind,
                path: full.join("::"),
            });
        } else if full.ends_with(&["Command".to_owned(), "new".to_owned()])
            && called
            && let Some(Tok::Str(program)) = toks.get(j + 1)
        {
            // `Command::new("rm")`, `Command::new("/bin/mv")`, `Command::new("del.exe")`.
            let name = program
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(program)
                .to_ascii_lowercase();
            let name = name.strip_suffix(".exe").unwrap_or(&name);
            if SHELL_FILE_COMMANDS.contains(&name) {
                out.push(MoveCall {
                    line,
                    kind: "shell_command",
                    path: format!("{}(\"{program}\")", full.join("::")),
                });
            }
        }
    }
    Ok(out)
}

/// Move calls in one source file (path relative to the workspace root, `/`-separated) that [`MOVE_ALLOWED`]
/// does not allow there, as `file:line: call` strings.
pub fn move_calls(file: &str, text: &str) -> Vec<String> {
    let allowed: Vec<&str> = MOVE_ALLOWED
        .iter()
        .filter(|(prefix, _, _)| {
            file == *prefix || (prefix.ends_with('/') && file.starts_with(prefix))
        })
        .flat_map(|(_, calls, _)| calls.iter().copied())
        .collect();
    let calls = match find_move_calls(text) {
        Ok(c) => c,
        Err(e) => return vec![format!("{file}: cannot be parsed for the move check: {e}")],
    };
    calls
        .into_iter()
        .filter(|c| !allowed.contains(&c.kind))
        .map(|c| {
            format!(
                "{file}:{}: `{}` ({}) is not allowed here (ADR-0004/0061/0062: only ps-export renames user \
                 files; see MOVE_ALLOWED in xtask/src/layering.rs)",
                c.line, c.kind, c.path
            )
        })
        .collect()
}

/// The files the move check reads: `crates/*/{src,benches,examples}/**/*.rs`, `crates/*/build.rs`,
/// `xtask/src/**/*.rs` and `xtask/build.rs`.
fn move_checked_files(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut dirs = vec![root.join("xtask/src")];
    let mut files = vec![root.join("xtask/build.rs")];
    for e in std::fs::read_dir(root.join("crates"))? {
        let c = e?.path();
        for d in ["src", "benches", "examples"] {
            dirs.push(c.join(d));
        }
        files.push(c.join("build.rs"));
    }
    let mut out: Vec<_> = files.into_iter().filter(|f| f.is_file()).collect();
    let mut stack: Vec<_> = dirs.into_iter().filter(|d| d.is_dir()).collect();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn move_violations(root: &Path) -> Result<Vec<String>> {
    let mut out: Vec<String> = MOVE_ALLOWED
        .iter()
        .flat_map(|(file, calls, _)| calls.iter().map(move |c| (file, c)))
        .filter(|(_, c)| !MOVE_CALLS.contains(c))
        .map(|(file, c)| format!("MOVE_ALLOWED entry {file}: unknown call kind `{c}`"))
        .collect();
    for p in move_checked_files(root)? {
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&p)?;
        out.extend(move_calls(&rel, &text));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_create_in_scan_sources_is_rejected() {
        let snippet = "fn f() {\n    let f = std::fs::File::create(\"x\").unwrap();\n}\n";
        let v = forbidden_calls("crates/ps-scan/src/bad.rs", snippet);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("bad.rs:2") && v[0].contains("File::create"));
        assert!(forbidden_calls("ok.rs", "OpenOptions::new().read(true).open(p)").is_empty());
        // Comments are not code.
        assert!(forbidden_calls("ok.rs", "// never fs::rename here").is_empty());
    }

    #[test]
    fn ui_crates_must_not_depend_on_engine_crates() {
        let v = facade_violations("ui-workspace", &["ps-service", "ps-catalog", "ps-model"]);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("ps-catalog"));
        assert!(facade_violations("photosquirrel", &["ps-service", "ps-media"]).is_empty());
    }

    #[test]
    fn imported_names_and_groups_are_followed() {
        let src = "use std::fs::{rename, remove_file as rm};\nfn f() {\n    rename(a, b).ok();\n    rm(p).ok();\n}\n";
        let v = find_move_calls(src).expect("parse");
        let at: Vec<(usize, &str)> = v.iter().map(|c| (c.line, c.kind)).collect();
        assert!(at.contains(&(3, "fs::rename")), "{v:?}");
        assert!(at.contains(&(4, "remove_file")), "{v:?}");
        assert_eq!(
            move_calls("crates/ps-service/src/x.rs", src).len(),
            4,
            "the imports and both calls"
        );
        // `use std::fs as f;`, `use std::{fs, io};`, `use std::fs::*;`, a leading `::`.
        for src in [
            "use std::fs as f;\nfn g() { f::rename(a, b).ok(); }",
            "use std::{fs, io};\nfn g() { fs::remove_dir_all(a).ok(); }",
            "use std::fs::*;\nfn g() { remove_file(a).ok(); }",
            "fn g() { ::std::fs::hard_link(a, b).ok(); }",
            "fn g() { assert!(std::fs::rename(a, b).is_ok()); }",
        ] {
            assert!(!find_move_calls(src).expect("parse").is_empty(), "{src}");
        }
    }

    #[test]
    fn unlink_calls_are_caught() {
        for src in [
            "fn g() { unsafe { libc::unlink(p) }; }",
            "fn g() { rustix::fs::unlinkat(CWD, p, AtFlags::empty()).ok(); }",
            "use rustix::fs::unlink;\nfn g() { unlink(p).ok(); }",
            "fn g() { nix::unistd::unlink(p).ok(); }",
            "use nix::unistd;\nfn g() { unistd::unlinkat(None, p, f).ok(); }",
        ] {
            let v = find_move_calls(src).expect("parse");
            assert!(v.iter().any(|c| c.kind == "unlink"), "{src}: {v:?}");
            assert!(
                !move_calls("crates/ps-export/src/quarantine/fsops.rs", src).is_empty(),
                "{src}"
            );
        }
        let win = "fn g() { unsafe { MoveFileExW(a, b, MOVEFILE_REPLACE_EXISTING) }; }";
        assert_eq!(move_calls("crates/ps-export/src/execute.rs", win).len(), 1);
        assert!(move_calls("crates/ps-export/src/quarantine/fsops.rs", win).is_empty());
    }

    #[test]
    fn fs_copy_outside_ps_export_is_rejected() {
        let src = "fn g() { std::fs::copy(a, b).unwrap(); }";
        assert_eq!(move_calls("crates/ps-scan/src/x.rs", src).len(), 1);
        assert_eq!(move_calls("crates/ps-service/src/x.rs", src).len(), 1);
        let src = "use tokio::fs::copy;\nasync fn g() { copy(a, b).await.ok(); }";
        assert!(!move_calls("crates/ps-service/src/x.rs", src).is_empty());
        assert!(
            move_calls(
                "xtask/src/package.rs",
                "fn g() { std::fs::copy(a, b).ok(); }"
            )
            .is_empty()
        );
    }

    #[test]
    fn strings_and_comments_are_not_code() {
        // A `//` inside a string literal does not hide the call after it on the line.
        let src = "fn g() { let u = \"http://x\"; std::fs::rename(a, b).ok(); }";
        assert_eq!(move_calls("crates/ps-service/src/x.rs", src).len(), 1);
        // A call named in a string or a comment is not a call.
        let src = "fn g() { log::info!(\"never std::fs::rename here\"); } // fs::remove_file(p)\n/* fs::rename */";
        assert!(move_calls("crates/ps-service/src/x.rs", src).is_empty());
        // A method of another type with the same name is not the free function.
        assert!(move_calls("x.rs", "fn g() { table.rename(a); }").is_empty());
        // The scanner's read-only check reads code tokens too.
        assert_eq!(
            forbidden_calls("x.rs", "fn g() { let u = \"a//b\"; File::create(p); }").len(),
            1
        );
    }

    #[test]
    fn test_items_are_skipped_but_not_cfg_not_test() {
        let src = "#[cfg(test)]\nmod tests {\n    fn t() { std::fs::remove_file(p).ok(); }\n}\n";
        assert!(move_calls("crates/ps-service/src/x.rs", src).is_empty());
        let src = "#[cfg(not(test))]\nfn real() { std::fs::remove_file(p).ok(); }\n";
        assert_eq!(move_calls("crates/ps-service/src/x.rs", src).len(), 1);
    }

    #[test]
    fn fs_err_calls_are_caught() {
        for (src, kind) in [
            ("fn g() { fs_err::remove_file(p).ok(); }", "remove_file"),
            ("fn g() { fs_err::rename(a, b).ok(); }", "fs::rename"),
            (
                "fn g() { fs_err::remove_dir_all(p).ok(); }",
                "remove_dir_all",
            ),
            ("fn g() { fs_err::hard_link(a, b).ok(); }", "hard_link"),
            ("fn g() { fs_err::copy(a, b).ok(); }", "fs::copy"),
            (
                "use fs_err as fs;\nfn g() { fs::remove_dir(p).ok(); }",
                "remove_dir",
            ),
            (
                "use fs_err::remove_file;\nfn g() { remove_file(p).ok(); }",
                "remove_file",
            ),
            (
                "async fn g() { fs_err::tokio::remove_file(p).await.ok(); }",
                "remove_file",
            ),
        ] {
            let v = find_move_calls(src).expect("parse");
            assert!(v.iter().any(|c| c.kind == kind), "{src}: {v:?}");
            assert!(
                !move_calls("crates/ps-service/src/x.rs", src).is_empty(),
                "{src}"
            );
        }
        // Reading through fs_err is fine.
        assert!(move_calls("x.rs", "fn g() { fs_err::read(p).ok(); }").is_empty());
    }

    #[test]
    fn remove_dir_all_crate_is_caught() {
        for src in [
            "fn g() { remove_dir_all::remove_dir_all(p).ok(); }",
            "use remove_dir_all::remove_dir_contents;\nfn g() { remove_dir_contents(p).ok(); }",
            "use remove_dir_all::*;\nfn g() { ensure_empty_dir(p).ok(); }",
        ] {
            let v = find_move_calls(src).expect("parse");
            assert!(v.iter().any(|c| c.kind == "remove_dir_all"), "{src}: {v:?}");
            assert!(
                !move_calls("crates/ps-service/src/x.rs", src).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn shell_file_commands_are_caught() {
        for src in [
            "fn g() { std::process::Command::new(\"rm\").arg(p).status().ok(); }",
            "use std::process::Command;\nfn g() { Command::new(\"mv\").args([a, b]).status().ok(); }",
            "fn g() { Command::new(\"rmdir\").arg(p).status().ok(); }",
            "fn g() { Command::new(\"del\").arg(p).status().ok(); }",
            "fn g() { Command::new(\"/bin/rm\").arg(p).status().ok(); }",
            "fn g() { Command::new(r\"C:\\Windows\\del.EXE\").status().ok(); }",
            "async fn g() { tokio::process::Command::new(\"rm\").status().await.ok(); }",
        ] {
            let v = find_move_calls(src).expect("parse");
            assert!(v.iter().any(|c| c.kind == "shell_command"), "{src}: {v:?}");
            // Allowed nowhere, not even in the move primitive.
            assert_eq!(
                move_calls("crates/ps-export/src/quarantine/fsops.rs", src).len(),
                1,
                "{src}"
            );
            assert_eq!(move_calls("xtask/src/package.rs", src).len(), 1, "{src}");
        }
        for src in [
            "fn g() { Command::new(\"cargo\").arg(\"rm\").status().ok(); }",
            "fn g() { Command::new(\"rmx\").status().ok(); }",
            "fn g() { log::info!(\"Command::new(rm)\"); }",
        ] {
            assert!(
                move_calls("crates/ps-service/src/x.rs", src).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn tempfile_persist_is_caught_but_not_persist_noclobber() {
        for src in [
            "fn g() { let t = tempfile::NamedTempFile::new()?; t.persist(p)?; }",
            "fn g() { tempfile::NamedTempFile::persist(t, p)?; }",
            "use tempfile::TempPath;\nfn g() { TempPath::persist(t, p)?; }",
            "fn g() { make_temp()\n    .persist(p)?; }",
        ] {
            let v = find_move_calls(src).expect("parse");
            assert!(v.iter().any(|c| c.kind == "persist"), "{src}: {v:?}");
            assert_eq!(
                move_calls("crates/ps-export/src/copy.rs", src).len(),
                1,
                "{src}"
            );
        }
        for src in [
            "fn g() { t.persist_noclobber(p)?; }",
            "fn g() { tempfile::NamedTempFile::persist_noclobber(t, p)?; }",
            "fn g() { t.keep()?; }",
        ] {
            assert!(
                move_calls("crates/ps-export/src/copy.rs", src).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn only_pure_cfg_test_is_skipped() {
        for src in [
            "#[cfg(any(test, feature = \"testing\"))]\nfn helper() { std::fs::remove_file(p).ok(); }\n",
            "#[cfg(any(feature = \"testing\", test))]\nmod t {\n    fn h() { std::fs::rename(a, b).ok(); }\n}\n",
            "#[cfg(feature = \"test\")]\nfn helper() { std::fs::remove_file(p).ok(); }\n",
            "#[cfg(not(test))]\nfn real() { std::fs::remove_file(p).ok(); }\n",
            "fn f() {\n    #[cfg(any(test, debug_assertions))]\n    let x = std::fs::remove_file(p);\n}\n",
        ] {
            assert_eq!(
                move_calls("crates/ps-service/src/x.rs", src).len(),
                1,
                "{src}"
            );
        }
        for src in [
            "#[cfg(test)]\nfn helper() { std::fs::remove_file(p).ok(); }\n",
            "#[cfg( test )]\nmod tests {\n    fn t() { std::fs::remove_file(p).ok(); }\n}\n",
        ] {
            assert!(
                move_calls("crates/ps-service/src/x.rs", src).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn build_scripts_and_xtask_are_checked() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let files = move_checked_files(&root).expect("files");
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(rel.iter().any(|p| p == "xtask/src/layering.rs"), "{rel:?}");
        assert!(rel.iter().any(|p| p.starts_with("crates/ps-hash/benches/")));
        assert!(
            rel.iter()
                .any(|p| p.starts_with("crates/ps-fixtures/examples/"))
        );
        // A build script is a checked file like any other.
        let v = move_calls(
            "crates/ps-media/build.rs",
            "fn main() { std::fs::remove_file(\"x\").ok(); }",
        );
        assert_eq!(v.len(), 1, "{v:?}");
        let v = move_calls("xtask/src/ci.rs", "fn f() { std::fs::rename(a, b).ok(); }");
        assert_eq!(v.len(), 1, "{v:?}");
    }

    #[test]
    fn rename_outside_the_quarantine_module_is_rejected() {
        let snippet = "fn f() {\n    std::fs::rename(a, b).unwrap();\n}\n";
        let v = move_calls("crates/ps-service/src/cleanup.rs", snippet);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("cleanup.rs:2") && v[0].contains("fs::rename"));
        // Elsewhere in ps-export too: only the quarantine primitive renames user files.
        assert_eq!(
            move_calls("crates/ps-export/src/execute.rs", snippet).len(),
            1
        );
        assert_eq!(
            move_calls("crates/ps-export/src/quarantine/run.rs", snippet).len(),
            1
        );
        // Not even in the move primitive: a plain rename replaces an existing target (ADR-0062).
        assert_eq!(
            move_calls("crates/ps-export/src/quarantine/fsops.rs", snippet).len(),
            1
        );
        // Unlinking a name is allowed only in the primitive (link+unlink); never elsewhere in the module.
        let rm = "std::fs::remove_file(p)?;";
        assert!(move_calls("crates/ps-export/src/quarantine/fsops.rs", rm).is_empty());
        assert_eq!(
            move_calls("crates/ps-export/src/quarantine/undo.rs", rm).len(),
            1
        );
        let rm_all = "std::fs::remove_dir_all(p)?;";
        let v = move_calls("crates/ps-export/src/quarantine/fsops.rs", rm_all);
        assert!(v.len() == 1 && v[0].contains("remove_dir_all"), "{v:?}");
        assert!(
            move_calls(
                "crates/ps-export/src/quarantine/fsops.rs",
                "fs::remove_dir(p)?;"
            )
            .is_empty()
        );
        assert!(
            move_calls(
                "crates/ps-export/src/quarantine/run.rs",
                "move_no_replace(a, b)"
            )
            .is_empty()
        );
        assert!(move_calls("x.rs", "// fs::rename in a comment").is_empty());
    }

    #[test]
    fn workspace_sources_pass_the_move_check() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = move_violations(&root).expect("scan");
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn ps_scan_sources_pass() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/ps-scan/src");
        let v = read_only_violations(&root).expect("scan");
        assert!(v.is_empty(), "{v:#?}");
    }
}
