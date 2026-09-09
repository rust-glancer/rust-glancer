use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn source_documentation_tokens_cover_links_and_rust_examples() {
    let fixture = LspEngineFixture::initialized(
        r###"
        //- /Cargo.toml
        [package]
        name = "doc_tokens"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        //! See [`Profile`].
        pub struct Profile;
        /// 🦀 [profile][target] and [`Self::id`].
        ///
        /// [target]: crate::Profile
        ///
        /// ```rust,no_run,edition2024
        /// # let hidden = 1;
        /// let café = "🦀";
        /// café.len();
        /// ```
        pub struct User {
            /// [`Profile`]
            pub id: Profile,
        }
        /** ~~~compile_fail
        struct Example<T> { value: T }
        ~~~ */
        pub enum State {
            /// [`Self::Ready`]
            Ready,
        }
        #[doc = "```\nlet n = 12;\n```"]
        pub struct Escaped;
        #[doc = r#"```ignore
        ##[derive(Clone)]
        struct Example;
        ```"#]
        pub struct Raw;
        /// [profile
        /// name](Profile)
        pub struct Multiline;
        /// $range_start$[profile][target]$range_end$
        ///
        /// [target]: Profile
        pub struct RangeDocs;
        pub enum Event {
            User {
                /// [`Profile`]
                profile: Profile,
            },
            Profile(/** [`Profile`] */ Profile),
        }
        pub struct Wrapped(/** [`Self`] */ pub Profile);
        "###,
    )
    .await;
    fixture
        .check(
            &[
                LspQuery::semantic_tokens("documentation tokens", "src/lib.rs", None),
                LspQuery::semantic_tokens(
                    "range with external reference definition",
                    "src/lib.rs",
                    Some(("range_start", "range_end")),
                ),
            ],
            expect![[r##"
                documentation tokens
                - 0:8-0:19 struct.documentation "[`Profile`]"
                - 2:7-2:24 struct.documentation "[profile][target]"
                - 2:29-2:41 property.documentation "[`Self::id`]"
                - 7:6-7:9 keyword.documentation "let"
                - 7:10-7:16 variable.documentation "hidden"
                - 7:17-7:18 operator.documentation "="
                - 7:19-7:20 number.documentation "1"
                - 7:20-7:21 operator.documentation ";"
                - 8:4-8:7 keyword.documentation "let"
                - 8:8-8:12 variable.documentation "café"
                - 8:13-8:14 operator.documentation "="
                - 8:15-8:19 string.documentation "\"🦀\""
                - 8:19-8:20 operator.documentation ";"
                - 9:4-9:8 variable.documentation "café"
                - 9:8-9:9 operator.documentation "."
                - 9:9-9:12 method.documentation "len"
                - 9:12-9:13 operator.documentation "("
                - 9:13-9:14 operator.documentation ")"
                - 9:14-9:15 operator.documentation ";"
                - 12:8-12:19 struct.documentation "[`Profile`]"
                - 16:0-16:6 keyword.documentation "struct"
                - 16:7-16:14 struct.documentation "Example"
                - 16:14-16:15 operator.documentation "<"
                - 16:15-16:16 typeParameter.documentation "T"
                - 16:16-16:17 operator.documentation ">"
                - 16:18-16:19 operator.documentation "{"
                - 16:20-16:25 property.documentation "value"
                - 16:25-16:26 operator.documentation ":"
                - 16:27-16:28 type.documentation "T"
                - 16:29-16:30 operator.documentation "}"
                - 19:8-19:23 enumMember.documentation "[`Self::Ready`]"
                - 22:14-22:17 keyword.documentation "let"
                - 22:18-22:19 variable.documentation "n"
                - 22:20-22:21 operator.documentation "="
                - 22:22-22:24 number.documentation "12"
                - 22:24-22:25 operator.documentation ";"
                - 25:1-25:2 operator.documentation "#"
                - 25:2-25:3 operator.documentation "["
                - 25:3-25:9 variable.documentation "derive"
                - 25:9-25:10 operator.documentation "("
                - 25:10-25:15 variable.documentation "Clone"
                - 25:15-25:16 operator.documentation ")"
                - 25:16-25:17 operator.documentation "]"
                - 26:0-26:6 keyword.documentation "struct"
                - 26:7-26:14 struct.documentation "Example"
                - 26:14-26:15 operator.documentation ";"
                - 29:4-29:12 struct.documentation "[profile"
                - 30:4-30:18 struct.documentation "name](Profile)"
                - 32:4-32:21 struct.documentation "[profile][target]"
                - 38:12-38:23 struct.documentation "[`Profile`]"
                - 41:16-41:27 struct.documentation "[`Profile`]"
                - 43:23-43:31 struct.documentation "[`Self`]"

                range with external reference definition
                - 32:4-32:21 struct.documentation "[profile][target]"
            "##]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn documentation_ranges_keep_reference_definitions_in_the_other_module_file() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "doc_module_ranges"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Profile;
        /// $outer_start$[profile][inner-target]$outer_end$
        ///
        /// [outer-target]: crate::Profile
        pub mod api;

        //- /src/api.rs
        //! $inner_start$[profile][outer-target]$inner_end$
        //!
        //! [inner-target]: crate::Profile
        "#,
    )
    .await;
    fixture
        .check(
            &[
                LspQuery::semantic_tokens(
                    "outer link with inner reference definition",
                    "src/lib.rs",
                    Some(("outer_start", "outer_end")),
                ),
                LspQuery::semantic_tokens(
                    "inner link with outer reference definition",
                    "src/api.rs",
                    Some(("inner_start", "inner_end")),
                ),
            ],
            expect![[r#"
                outer link with inner reference definition
                - 1:4-1:27 struct.documentation "[profile][inner-target]"

                inner link with outer reference definition
                - 0:4-0:27 struct.documentation "[profile][outer-target]"
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn source_documentation_tracks_unsaved_comments_and_attributes() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "doc_edits"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Profile;
        /// [`Profile`]
        pub mod api {
            //! [`Profile`]
            pub struct Profile;
        }
        #[doc = "Old documentation."]
        pub struct User;
        "#,
    )
    .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let current = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                &r#"
// An edit moves every declaration and lengthens the module's outer docs.
pub struct Profile;
/// Longer outer docs select [`Pro$outer$file`] in the enclosing module.
pub mod api {
    //! [`Pro$inner$file`] selects this module's definition.
    pub struct Profile;
}
#[doc = "🦀 [`Pr\u{6f}$attribute$file`]"]
/// ```rust
/// let value = "🦀";
/// ```
pub struct User;
"#
                .replace('\n', "\r\n"),
            ),
        )
        .await;
    fixture
        .check_dirty(
            &current,
            &[
                LspQuery::goto_definition("edited outer scope", "outer"),
                LspQuery::goto_definition("edited inner scope", "inner"),
                LspQuery::goto_definition("edited attribute owner", "attribute"),
                LspQuery::hover("edited attribute hover", "attribute"),
                LspQuery::semantic_tokens("edited documentation tokens", "src/lib.rs", None),
            ],
            expect![[r#"
                edited outer scope
                - /src/lib.rs:2:11-2:18

                edited inner scope
                - /src/lib.rs:6:15-6:22

                edited attribute owner
                - /src/lib.rs:2:11-2:18

                edited attribute hover
                - range: /src/lib.rs:8:12-8:28
                - markdown:
                  ```rust
                  doc_edits::Profile
                  ```

                  ```rust
                  pub struct Profile
                  ```

                edited documentation tokens
                - 3:29-3:40 struct.documentation "[`Profile`]"
                - 5:8-5:19 struct.documentation "[`Profile`]"
                - 8:12-8:28 struct.documentation "[`Pr\\u{6f}file`]"
                - 10:4-10:7 keyword.documentation "let"
                - 10:8-10:13 variable.documentation "value"
                - 10:14-10:15 operator.documentation "="
                - 10:16-10:20 string.documentation "\"🦀\""
                - 10:20-10:21 operator.documentation ";"
            "#]],
        )
        .await;
    fixture.shutdown().await;
}
