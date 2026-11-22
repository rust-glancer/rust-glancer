use std::{path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Ok(root) = manifest.join("../..").canonicalize() else {
        return;
    };
    if git(&root, &["rev-parse", "--show-toplevel"])
        .map(|path| path.trim().to_owned())
        .as_deref()
        != root.to_str()
    {
        return;
    }
    let Some(revision) = git(&root, &["rev-parse", "--short=12", "HEAD"]) else {
        return;
    };
    let Some(status) = git(
        &root,
        &["status", "--porcelain", "--untracked-files=normal"],
    ) else {
        return;
    };
    let branch = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let state = if status.is_empty() { "clean" } else { "dirty" };
    let mut source = format!("{}, {state}", revision.trim());
    if let Some(branch) = branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| *branch != "main")
    {
        source.push_str(", ");
        source.push_str(branch);
    }
    println!("cargo:rustc-env=RUST_GLANCER_BUILD_SOURCE={source}");

    // Git metadata changes on commits and branch switches even when the compiled files did
    // not change. Track source files separately so edits refresh the recorded dirty state.
    for name in ["HEAD", "index", "refs/heads", "packed-refs"] {
        if let Some(path) = git(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        ) && Path::new(path.trim()).exists()
        {
            println!("cargo:rerun-if-changed={}", path.trim());
        }
    }
    if let Some(files) = git(
        &root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    ) {
        for file in files.split_terminator('\0') {
            let path = root.join(file);
            if !file.contains(['\r', '\n']) && path.is_file() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()
}
