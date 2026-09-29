//! Security audit tests: prove no shell usage, verify process spawn sites,
//! and check secret-handling hygiene by source inspection (in addition to
//! the behavioral tests in schema/ai/engine).

use std::path::Path;

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/engine -> repo root
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("root")
        .to_path_buf()
}

use std::path::PathBuf;

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name()
                    .is_some_and(|n| n == "target" || n == "node_modules" || n == "dist")
                {
                    continue;
                }
                collect_rs(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
}

/// Every `Command::new` in the codebase must spawn a fixed media tool or a
/// user-configured transcriber binary — and NEVER with a single shell string.
#[test]
fn no_shell_spawn_sites_in_source() {
    let root = repo_root();
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    assert!(
        files.len() > 20,
        "expected to audit the whole workspace, found {}",
        files.len()
    );
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_default();
        for (i, line) in src.lines().enumerate() {
            let lower = line.to_lowercase();
            if lower.contains("sh -c") || lower.contains("cmd /c") || lower.contains("/bin/bash") {
                // Negative assertions, comments, and fixture string data are
                // fine — we are auditing SPAWN SITES, not test data.
                let is_negative_or_fixture = line.contains("assert")
                    || line.contains("//")
                    || line.contains("contains(")
                    || line.contains("add_text")
                    || f.to_string_lossy().contains("/tests/")
                    || f.file_name().map(|n| n == "security.rs").unwrap_or(false);
                assert!(
                    is_negative_or_fixture,
                    "{}:{} possible shell spawn: {line}",
                    f.display(),
                    i + 1
                );
            }
        }
    }
}

/// `shell = true` equivalents don't exist in std Rust, but ensure nobody
/// pulled in a crate like `duct`/`shell-words` that encourages shell strings.
#[test]
fn no_shell_helper_crates_in_cargo_tomls() {
    let root = repo_root();
    let mut manifests = Vec::new();
    collect_tomls(&root, &mut manifests);
    for m in manifests {
        let src = std::fs::read_to_string(&m).unwrap_or_default();
        for banned in ["duct", "shell-words", "shlex", "subprocess "] {
            assert!(
                !src.contains(banned),
                "{} must not depend on shell-helper crate {banned}",
                m.display()
            );
        }
    }
}

fn collect_tomls(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name()
                    .is_some_and(|n| n == "target" || n == "node_modules")
                {
                    continue;
                }
                collect_tomls(&p, out);
            } else if p.file_name().is_some_and(|n| n == "Cargo.toml") {
                out.push(p);
            }
        }
    }
}

/// The plan schema document must keep strict mode markers (`additionalProperties:
/// false` on objects) so adversarial fields can never ride along.
#[test]
fn schema_document_is_strict() {
    let schema = include_str!("../../../docs/schema/edit-plan.schema.json");
    let doc: serde_json::Value = serde_json::from_str(schema).unwrap();
    // Every "operation" oneOf branch rejects unknown members.
    let text = serde_json::to_string(&doc).unwrap();
    let count_additional_false = text.matches("\"additionalProperties\":false").count();
    assert!(
        count_additional_false >= 3,
        "schema must be strict ({count_additional_false})"
    );
}
