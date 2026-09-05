# Changelog

## [0.2.1](https://github.com/rust-glancer/rust-glancer/compare/v0.2.0...v0.2.1) (2026-09-05)


### Features

* Implement folding ([#211](https://github.com/rust-glancer/rust-glancer/issues/211)) ([8aed80e](https://github.com/rust-glancer/rust-glancer/commit/8aed80e34f2d0cf0585436f74e68a77f317b6217))


### Bug Fixes

* Fallback on rootUri if no workspace in initial ([#207](https://github.com/rust-glancer/rust-glancer/issues/207)) ([63e67a0](https://github.com/rust-glancer/rust-glancer/commit/63e67a07becf7310ebe8dea7ba9b1d105f9ed4ae))

## [0.2.0](https://github.com/rust-glancer/rust-glancer/compare/v0.1.1...v0.2.0) (2026-08-31)


### Features

* Add publishing to OVSX ([#202](https://github.com/rust-glancer/rust-glancer/issues/202)) ([5cc1adc](https://github.com/rust-glancer/rust-glancer/commit/5cc1adc3a21da2335ab2226ea8dec45a173b370a))
* Code actions & better current overlay ([#187](https://github.com/rust-glancer/rust-glancer/issues/187)) ([d6beb3a](https://github.com/rust-glancer/rust-glancer/commit/d6beb3aa6889c77936a36c9c2deb2b011fa67e36))
* Fix primitive methods resolution ([#188](https://github.com/rust-glancer/rust-glancer/issues/188)) ([de58056](https://github.com/rust-glancer/rust-glancer/commit/de58056d8128bc9a0bf6fe2c8092b7f634761ad8))
* Lower peak memory and optimize speed of indexing ([#199](https://github.com/rust-glancer/rust-glancer/issues/199)) ([a7aea4f](https://github.com/rust-glancer/rust-glancer/commit/a7aea4fa22efdef92d781f87ad1d6db4668ad135))
* Make mimalloc the default allocator ([#201](https://github.com/rust-glancer/rust-glancer/issues/201)) ([aced3cd](https://github.com/rust-glancer/rust-glancer/commit/aced3cd14ad7a021982c0452777d4ae5f324ea5b))
* Model extern items and proc macro crate ([#176](https://github.com/rust-glancer/rust-glancer/issues/176)) ([b63c615](https://github.com/rust-glancer/rust-glancer/commit/b63c61536cd6a58ec0281f208cfeab17615c4781))
* Optimize indexing for packages with many targets ([#172](https://github.com/rust-glancer/rust-glancer/issues/172)) ([110b2df](https://github.com/rust-glancer/rust-glancer/commit/110b2df8aba6c2db340041aa03a6bd6c7102f37b))
* Per-crate shards for defmap/semantic artifacts ([#191](https://github.com/rust-glancer/rust-glancer/issues/191)) ([297b928](https://github.com/rust-glancer/rust-glancer/commit/297b928573c612e9be9be02281ece6d53fd02345))
* Support build scripts ([#180](https://github.com/rust-glancer/rust-glancer/issues/180)) ([019305a](https://github.com/rust-glancer/rust-glancer/commit/019305aa12d59505828081c0a0792ade62584b3c))
* Support macro-generated modules ([#178](https://github.com/rust-glancer/rust-glancer/issues/178)) ([3454d2a](https://github.com/rust-glancer/rust-glancer/commit/3454d2a3ddf9797efda60a238c9f5403dec906e4))
* Trigger release please ([e118a0b](https://github.com/rust-glancer/rust-glancer/commit/e118a0b565b20506d51408740ab05c186f6efa0f))
* VS Code extension menu ([#190](https://github.com/rust-glancer/rust-glancer/issues/190)) ([ea0696c](https://github.com/rust-glancer/rust-glancer/commit/ea0696cc1091823a8611c09f38cf6cd21c5e3333))
* Windows support ([#183](https://github.com/rust-glancer/rust-glancer/issues/183)) ([325a244](https://github.com/rust-glancer/rust-glancer/commit/325a24455b9e0876a692eafaaa5aa63f741bcab8))
* Zed extension ([#186](https://github.com/rust-glancer/rust-glancer/issues/186)) ([c93b55e](https://github.com/rust-glancer/rust-glancer/commit/c93b55e966cb0d0c2cc27c1fcf3b17142d34f982))


### Bug Fixes

* disable jemalloc on OpenBSD ([#193](https://github.com/rust-glancer/rust-glancer/issues/193)) ([a11d024](https://github.com/rust-glancer/rust-glancer/commit/a11d024e4bf9a543ae9ef5ec3bc7f09098010b09))
* Do not include generated sources to defmap artifacts ([#203](https://github.com/rust-glancer/rust-glancer/issues/203)) ([c6a5d8e](https://github.com/rust-glancer/rust-glancer/commit/c6a5d8ea53cde724fa221e5c7041beb2b19e4d10))
* More candidate filtering and cancellation ([#204](https://github.com/rust-glancer/rust-glancer/issues/204)) ([18d4642](https://github.com/rust-glancer/rust-glancer/commit/18d4642b86daecca415c7422c6401957aca0bbb9))
* Properly report server version ([#196](https://github.com/rust-glancer/rust-glancer/issues/196)) ([d392483](https://github.com/rust-glancer/rust-glancer/commit/d392483c5804ad70becbd29b2067000140b77658))
* Reduce memory fragmentation after initial indexing ([#192](https://github.com/rust-glancer/rust-glancer/issues/192)) ([b170357](https://github.com/rust-glancer/rust-glancer/commit/b1703579e46aad1dab64e86caa8eb540f840d237))
* Use older glibc for releases ([#181](https://github.com/rust-glancer/rust-glancer/issues/181)) ([7c68f1a](https://github.com/rust-glancer/rust-glancer/commit/7c68f1afb5ace31026013dee3a75a8ea4cf1684f))


### Miscellaneous Chores

* prepare 0.2.0 release ([ae9e812](https://github.com/rust-glancer/rust-glancer/commit/ae9e812a603330e57a20777f4139f42293674a22))

## [0.1.1](https://github.com/rust-glancer/rust-glancer/compare/v0.1.0...v0.1.1) (2026-08-20)


### Bug Fixes

* Last minute fixes (type inference) ([#168](https://github.com/rust-glancer/rust-glancer/issues/168)) ([16efa5b](https://github.com/rust-glancer/rust-glancer/commit/16efa5b05cc4db811fb7c35e2cfbe371daac742b))

## 0.1.0 (2026-08-20)


### Features

* Docs & release preparation ([#163](https://github.com/rust-glancer/rust-glancer/issues/163)) ([f6f4761](https://github.com/rust-glancer/rust-glancer/commit/f6f4761c89c989886fe5322a8bc593412f35a395))
* Indexing and performance optimizations ([#164](https://github.com/rust-glancer/rust-glancer/issues/164)) ([c4d0f74](https://github.com/rust-glancer/rust-glancer/commit/c4d0f74c4ec8ff2ce348f30df4bb2882c1eebd45))


### Bug Fixes

* Fix release please and extension name ([#165](https://github.com/rust-glancer/rust-glancer/issues/165)) ([058199b](https://github.com/rust-glancer/rust-glancer/commit/058199b3ca8ac31f61a0acc2a1bd98ac82537330))

## Changelog
