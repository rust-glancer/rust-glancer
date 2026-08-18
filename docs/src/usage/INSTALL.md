# Installation

Before anything else, `rust-src` is mandatory for the project to work correctly,
so don't forget to run `rustup component add rust-src` if you're not sure if you
have it installed.

Right now, the primary editor for Rust Glancer is VS Code.
It should be usable with other editors that natively support LSP, and you can
try following the instructions [rust-analyzer provides](https://rust-analyzer.github.io/book/installation.html).

Native support for more editors is expected in future.

When you install Rust Glancer, do not forget to turn rust-analyzer off. They should work
together just fine (I've done that and did not have any conflicts), but it's kind of
meaningless to run both.

## VS Code

You have two options:
1. [Install the extension from the official marketplace](https://marketplace.visualstudio.com/items?itemName=rust-glancer.rust-glancer).
2. Build and install VSIX from the repository.

The extension is maintained and will be updated, but given that VS Code extensions
often become targets of attacks nowadays, I'd probably recommend building from source
(or at least disabling auto-updates). Please do not forget to update it from time
to time though. There will be good things in updates (probably).

### Installing from VSIX

0. (Optional) Install [just](https://github.com/casey/just)
1. Clone the [repository](https://github.com/rust-glancer/rust-glancer)
2. Run `just package-vsix` (or go to `editors/code` and build via `npm`)
3. Open VS Code, navigate to extensions tab, click on `...` and choose `Install from VSIX`.
4. Install the extension.
5. ???
6. PROFIT
