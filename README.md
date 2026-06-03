# rust-glancer

[![CodSpeed](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json)](https://codspeed.io/rust-glancer/rust-glancer?utm_source=badge)

An incomplete-by-design LSP that trades completeness for speed and memory.
`rust-analyzer` is great, but it is just too heavy.

This project aims to get you 70% there, with most low-hanging fruit supported,
but not more.

See [SCOPE](docs/SCOPE.md) to see the idea behind the project, and [ARCHITECTURE](docs/ARCHITECTURE.md)
to understand how it works and how it's different from `rust-analyzer`.

## AI use disclaimer

This project was created with heavy use of LLMs. It is not vibe coded and it is not AI slop, though.
If you want to learn about AI journey, how it went wrong, and how it become right again, [see this PR](https://github.com/rust-glancer/rust-glancer/pull/78).

At no point were LLMs used as a replacement for a brain. So if you consider it to be slop, then it is _my_ slop.

Keep in mind, however, that I am not an LSP expert. I learn as I build it, so if something is not good enough, _eventually_ I will notice and fix this. It's all a part of journey, and not every code smell is caused by AI (though many certainly are).

## Acknowledgements

The `rust-analyzer` project is great, and some parts are heavily influenced by or even borrowed from there.
I truly believe that success of Rust as a language can be attributed, among other reasons, to having such
a cool LSP.

As a result, this project is not a _replacement_ for `rust-analyzer`; it is an _alternative_ for those
who are ready for some compromises.

## Contributing

See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md).

## License

Licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
