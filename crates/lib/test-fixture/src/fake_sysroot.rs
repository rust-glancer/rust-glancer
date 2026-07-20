pub(crate) struct FakeSysrootFile {
    pub(crate) relative_path: &'static str,
    pub(crate) contents: &'static str,
}

const FILES: &[FakeSysrootFile] = &[
    FakeSysrootFile {
        relative_path: "sysroot/library/core/Cargo.toml",
        contents: include_str!("../assets/fake_sysroot/core/Cargo.toml"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/lib.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/lib.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/array.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/array.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/fmt.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/fmt.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/iter/mod.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/iter/mod.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/iter/adapters/mod.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/iter/adapters/mod.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/macros/mod.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/macros/mod.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/ops.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/ops.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/option.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/option.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/prelude/mod.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/prelude/mod.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/result.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/result.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/core/src/slice.rs",
        contents: include_str!("../assets/fake_sysroot/core/src/slice.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/alloc/Cargo.toml",
        contents: include_str!("../assets/fake_sysroot/alloc/Cargo.toml"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/alloc/src/lib.rs",
        contents: include_str!("../assets/fake_sysroot/alloc/src/lib.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/alloc/src/string.rs",
        contents: include_str!("../assets/fake_sysroot/alloc/src/string.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/alloc/src/vec/mod.rs",
        contents: include_str!("../assets/fake_sysroot/alloc/src/vec/mod.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/std/Cargo.toml",
        contents: include_str!("../assets/fake_sysroot/std/Cargo.toml"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/std/src/lib.rs",
        contents: include_str!("../assets/fake_sysroot/std/src/lib.rs"),
    },
    FakeSysrootFile {
        relative_path: "sysroot/library/std/src/prelude/mod.rs",
        contents: include_str!("../assets/fake_sysroot/std/src/prelude/mod.rs"),
    },
];

pub(crate) fn files() -> &'static [FakeSysrootFile] {
    FILES
}
