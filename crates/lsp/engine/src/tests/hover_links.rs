use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn hover_links_resolve_in_the_documented_items_scope() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "hover_links"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Profile;
        pub fn load() {}

        mod api {
            use super::Profile as Account;
            /// [`Account`], [`super::load`], [profile](crate::Profile), and [account][id].
            ///
            /// [id]: Account
            ///
            /// [Rust](https://www.rust-lang.org/) and `let example = "[Account]";`.
            ///
            /// ```rust,no_run
            /// # let setup = "[Account]";
            /// let text = "[Account]";
            /// ```
            pub struct User;
        }
        pub use api::User;

        mod consumer {
            struct Account;
            fn load() {}
            pub fn demo(_: super::Us$hover$er) {}
        }
        "#,
    )
    .await;
    fixture
        .check(
            &[LspQuery::hover("links from a re-exported item", "hover")],
            expect![[r#"
                links from a re-exported item
                - range: /src/lib.rs:22:26-22:30
                - markdown:
                  ```rust
                  hover_links::api::User
                  ```

                  ```rust
                  pub struct User
                  ```

                  [`Account`](<file://$ROOT/src/lib.rs#L1,12>), [`super::load`](<file://$ROOT/src/lib.rs#L2,8>), [profile](<file://$ROOT/src/lib.rs#L1,12>), and [account](<file://$ROOT/src/lib.rs#L1,12>).

                  [id]: Account

                  [Rust](https://www.rust-lang.org/) and `let example = "[Account]";`.

                  ```rust
                  let text = "[Account]";
                  ```
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn hover_links_keep_outer_and_inner_module_scopes() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "hover_links"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Profile;
        /// Outer [`Profile`].
        pub mod in$inline$line {
            //! Inner [`Profile`] and parent [`super::Profile`].
            pub struct Profile;
        }
        /// Outer [`Profile`].
        pub mod out$outline$line;

        //- /src/outline.rs
        //! Inner [`Profile`] and parent [`super::Profile`].
        pub struct Profile;
        "#,
    )
    .await;
    fixture
        .check(
            &[
                LspQuery::hover("inline module documentation", "inline"),
                LspQuery::hover("out-of-line module documentation", "outline"),
            ],
            expect![[r#"
                inline module documentation
                - range: /src/lib.rs:2:8-2:14
                - markdown:
                  ```rust
                  hover_links::inline
                  ```

                  ```rust
                  mod inline
                  ```

                  Outer [`Profile`](<file://$ROOT/src/lib.rs#L1,12>).
                  Inner [`Profile`](<file://$ROOT/src/lib.rs#L5,16>) and parent [`super::Profile`](<file://$ROOT/src/lib.rs#L1,12>).

                out-of-line module documentation
                - range: /src/lib.rs:7:8-7:15
                - markdown:
                  ```rust
                  hover_links::outline
                  ```

                  ```rust
                  mod outline
                  ```

                  Outer [`Profile`](<file://$ROOT/src/lib.rs#L1,12>).
                  Inner [`Profile`](<file://$ROOT/src/outline.rs#L2,12>) and parent [`super::Profile`](<file://$ROOT/src/lib.rs#L1,12>).
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn hover_links_resolve_self_members_and_disambiguated_names() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "hover_links"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        /// [`Self`], [`Self::new()`], [`Self::name`], [`State::Ready`], and [`Factory::make`].
        pub struct Us$user$er { pub name: u32 }
        impl User {
            /// [`Self`] and [`Self::name`].
            pub fn n$method$ew() -> Self { User { name: 0 } }
        }
        pub enum State { Ready }
        pub trait Factory { fn make(); }
        pub struct Profile {}
        pub fn Profile() {}
        macro_rules! profile { () => {} }
        /// [type](struct@Profile), [function](fn@Profile), and [`profile!`].
        pub struct Acc$account$ount;
        /// [`User::new`] and [`Factory::make`].
        pub mod a$module$pi {}
        "#,
    )
    .await;
    fixture
        .check(
            &[
                LspQuery::hover("type and member links", "user"),
                LspQuery::hover("impl Self links", "method"),
                LspQuery::hover("disambiguated item links", "account"),
                LspQuery::hover("associated links in module docs", "module"),
            ],
            expect![[r#"
                type and member links
                - range: /src/lib.rs:1:11-1:15
                - markdown:
                  ```rust
                  hover_links::User
                  ```

                  ```rust
                  pub struct User {
                      pub name: u32,
                  }
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L2,12>), [`Self::new()`](<file://$ROOT/src/lib.rs#L5,12>), [`Self::name`](<file://$ROOT/src/lib.rs#L2,23>), [`State::Ready`](<file://$ROOT/src/lib.rs#L7,18>), and [`Factory::make`](<file://$ROOT/src/lib.rs#L8,24>).

                impl Self links
                - range: /src/lib.rs:4:11-4:14
                - markdown:
                  ```rust
                  hover_links::User::new
                  ```

                  ```rust
                  pub fn new() -> Self
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L2,12>) and [`Self::name`](<file://$ROOT/src/lib.rs#L2,23>).

                disambiguated item links
                - range: /src/lib.rs:12:11-12:18
                - markdown:
                  ```rust
                  hover_links::Account
                  ```

                  ```rust
                  pub struct Account
                  ```

                  [type](<file://$ROOT/src/lib.rs#L9,12>), [function](<file://$ROOT/src/lib.rs#L10,8>), and [`profile!`](<file://$ROOT/src/lib.rs#L11,14>).

                associated links in module docs
                - range: /src/lib.rs:14:8-14:11
                - markdown:
                  ```rust
                  hover_links::api
                  ```

                  ```rust
                  mod api
                  ```

                  [`User::new`](<file://$ROOT/src/lib.rs#L5,12>) and [`Factory::make`](<file://$ROOT/src/lib.rs#L8,24>).
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn hover_self_links_preserve_their_owner_identity() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "hover_links"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn Profile() {}
        /// [`Self`], [`Self::name`], and [`Self::new`].
        pub struct Pro$profile$file { pub name: u32 }
        impl Profile {
            pub fn new() -> Self { Profile { name: 0 } }
        }

        pub fn Factory() {}
        /// [`Self`] and [`Self::make`].
        pub trait Fac$factory$tory {
            /// [`Self`] and [`Self::make`].
            fn ma$make$ke() -> Self;
        }

        pub fn State() {}
        /// [`Self`] and [`Self::Ready`].
        pub enum St$state$ate { Ready }
        "#,
    )
    .await;
    fixture
        .check(
            &[
                LspQuery::hover("struct Self despite a same-named function", "profile"),
                LspQuery::hover("trait Self despite a same-named function", "factory"),
                LspQuery::hover("Self on an associated trait item", "make"),
                LspQuery::hover("enum Self and its variant", "state"),
            ],
            expect![[r#"
                struct Self despite a same-named function
                - range: /src/lib.rs:2:11-2:18
                - markdown:
                  ```rust
                  hover_links::Profile
                  ```

                  ```rust
                  pub struct Profile {
                      pub name: u32,
                  }
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L3,12>), [`Self::name`](<file://$ROOT/src/lib.rs#L3,26>), and [`Self::new`](<file://$ROOT/src/lib.rs#L5,12>).

                trait Self despite a same-named function
                - range: /src/lib.rs:9:10-9:17
                - markdown:
                  ```rust
                  hover_links::Factory
                  ```

                  ```rust
                  pub trait Factory
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L10,11>) and [`Self::make`](<file://$ROOT/src/lib.rs#L12,8>).

                Self on an associated trait item
                - range: /src/lib.rs:11:7-11:11
                - markdown:
                  ```rust
                  hover_links::Factory::make
                  ```

                  ```rust
                  fn make() -> Self
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L10,11>) and [`Self::make`](<file://$ROOT/src/lib.rs#L12,8>).

                enum Self and its variant
                - range: /src/lib.rs:16:9-16:14
                - markdown:
                  ```rust
                  hover_links::State
                  ```

                  ```rust
                  pub enum State {
                      Ready,
                  }
                  ```

                  [`Self`](<file://$ROOT/src/lib.rs#L17,10>) and [`Self::Ready`](<file://$ROOT/src/lib.rs#L17,18>).
            "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn hover_links_use_unsaved_destination_positions() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "hover_links"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod model;
        /// [`model::Profile`].
        pub struct Us$hover$er;

        //- /src/model.rs
        pub struct Profile;
        "#,
    )
    .await;
    fixture.did_open_saved("src/model.rs", 1).await;
    fixture
        .did_change_full(
            "src/model.rs",
            2,
            MarkedText::parse("// A comment inserted in the editor.\n\n    pub struct Profile;\n"),
        )
        .await;
    fixture
        .check(
            &[LspQuery::hover("link to a moved declaration", "hover")],
            expect![[r#"
                link to a moved declaration
                - range: /src/lib.rs:2:11-2:15
                - markdown:
                  ```rust
                  hover_links::User
                  ```

                  ```rust
                  pub struct User
                  ```

                  [`model::Profile`](<file://$ROOT/src/model.rs#L3,16>).
            "#]],
        )
        .await;
    fixture.shutdown().await;
}
