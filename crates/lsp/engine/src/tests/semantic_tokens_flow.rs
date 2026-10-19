use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn format_capture_tokens_follow_statement_wrappers_and_nested_arguments() {
    let fixture = LspEngineFixture::initialized_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "expanded_format_tokens"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! emit {
    ($template:literal) => {
        let _ = std::format_args!($template);
        let _ = std::format_args!($template);
    };
}
mod messages {
    use std::format_args as render;
    pub fn show(user: &str, width: usize) {
        emit!("{user}");
        let _ = render!("outer {}", render!("inner {user}"));
        std::cfg_select! {
            all() => { let _ = render!("{user:>width$}"); }
        }
    }
}
"#,
    )
    .await;
    fixture
        .check_queries(
            &[LspQuery::semantic_tokens(
                "expanded formatting",
                "src/lib.rs",
                None,
            )],
            expect![[r#"
                // expanded formatting
                9:15-9:16 formatSpecifier "{"
                9:16-9:20 variable "user"
                9:20-9:21 formatSpecifier "}"
                10:31-10:33 formatSpecifier "{}"
                10:51-10:52 formatSpecifier "{"
                10:52-10:56 variable "user"
                10:56-10:57 formatSpecifier "}"
                12:40-12:41 formatSpecifier "{"
                12:41-12:45 variable "user"
                12:45-12:47 formatSpecifier ":>"
                12:47-12:52 variable "width"
                12:52-12:54 formatSpecifier "$}"
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn format_capture_tokens_highlight_names_and_specifiers() {
    let fixture = LspEngineFixture::initialized_with_fake_sysroot(
        r###"
//- /Cargo.toml
[package]
name = "format_tokens"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::format_args as arguments;
macro_rules! render { ($template:literal) => { format!($template) }; }
macro_rules! twice { ($template:literal) => { (render!($template), render!($template)) }; }
pub fn demo(user: &str, width: usize, precision: usize, café: u32) {
    let _ = format!("hello {$range_start$user:>width$$range_end$}!");
    let _ = format_args!("{0:>width$.precision$}", 1.25);
    let _ = arguments!(r##"🦀 {{literal}} {user:?}"##);
    let _ = render!("escaped \u{7b}caf\u{e9}\u{7d}");
    let _ = twice!("repeat {user}");
    let _ = arguments!(r#"first
{user:>width$}
last"#);
    assert!(true, "bad {user}");
    let _ = format!("line\n{user}\t");
}
"###,
    )
    .await;
    fixture
        .check_queries(
            &[
                LspQuery::semantic_tokens("format string tokens", "src/lib.rs", None),
                LspQuery::semantic_tokens(
                    "range inside a format string",
                    "src/lib.rs",
                    Some(("range_start", "range_end")),
                ),
            ],
            expect![[r#"
                // format string tokens
                4:27-4:28 formatSpecifier "{"
                4:28-4:32 variable "user"
                4:32-4:34 formatSpecifier ":>"
                4:34-4:39 variable "width"
                4:39-4:41 formatSpecifier "$}"
                5:26-5:30 formatSpecifier "{0:>"
                5:30-5:35 variable "width"
                5:35-5:37 formatSpecifier "$."
                5:37-5:46 variable "precision"
                5:46-5:48 formatSpecifier "$}"
                6:42-6:43 formatSpecifier "{"
                6:43-6:47 variable "user"
                6:47-6:50 formatSpecifier ":?}"
                7:29-7:35 formatSpecifier "\\u{7b}"
                7:35-7:44 variable "caf\\u{e9}"
                7:44-7:50 formatSpecifier "\\u{7d}"
                8:27-8:28 formatSpecifier "{"
                8:28-8:32 variable "user"
                8:32-8:33 formatSpecifier "}"
                10:0-10:1 formatSpecifier "{"
                10:1-10:5 variable "user"
                10:5-10:7 formatSpecifier ":>"
                10:7-10:12 variable "width"
                10:12-10:14 formatSpecifier "$}"
                12:23-12:24 formatSpecifier "{"
                12:24-12:28 variable "user"
                12:28-12:29 formatSpecifier "}"
                13:27-13:28 formatSpecifier "{"
                13:28-13:32 variable "user"
                13:32-13:33 formatSpecifier "}"

                // range inside a format string
                4:28-4:32 variable "user"
                4:32-4:34 formatSpecifier ":>"
                4:34-4:39 variable "width"
                4:39-4:40 formatSpecifier "$"
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn format_capture_tokens_follow_unsaved_text() {
    let fixture = LspEngineFixture::initialized_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "edited_format_tokens"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn demo(user: &str, width: usize) {
    let _ = format!("🦀 {user:>width$}");
}
"#,
    )
    .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub fn demo(user: &str, width: usize) {
    let current = 1u64;
    let _ = format!("🦀 {current:>width$}!");
}
"#,
        )
        .await;
    fixture
        .check_dirty_queries(
            &dirty,
            &[LspQuery::semantic_tokens(
                "edited format string tokens",
                "src/lib.rs",
                None,
            )],
            expect![[r#"
                // edited format string tokens
                3:24-3:25 formatSpecifier "{"
                3:25-3:32 variable "current"
                3:32-3:34 formatSpecifier ":>"
                3:34-3:39 variable "width"
                3:39-3:41 formatSpecifier "$}"
            "#]],
        )
        .await;
    fixture.shutdown().await;
}
