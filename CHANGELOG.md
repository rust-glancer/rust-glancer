# Changelog

## [0.4.1](https://github.com/rust-glancer/rust-glancer/compare/v0.4.0...v0.4.1) (2026-11-21)


### Features

* **Ajika:** Application cutover ([#296](https://github.com/rust-glancer/rust-glancer/issues/296)) ([41aeb7a](https://github.com/rust-glancer/rust-glancer/commit/41aeb7af16ee0e5045e7de8fe5134a46c72c7b3e))
* **Ajika:** Detach body contents from spans ([#300](https://github.com/rust-glancer/rust-glancer/issues/300)) ([5d89bad](https://github.com/rust-glancer/rust-glancer/commit/5d89badb3d1739a4a643da9ffe71a68fe19527cb))
* **Ajika:** Introduce Ajika and migrate item tree ([#295](https://github.com/rust-glancer/rust-glancer/issues/295)) ([ad447f2](https://github.com/rust-glancer/rust-glancer/commit/ad447f27f716c64b6177b260caedf387d9fac31e))
* **Ajika:** Lazy associated member evaluation ([#305](https://github.com/rust-glancer/rust-glancer/issues/305)) ([f85de5b](https://github.com/rust-glancer/rust-glancer/commit/f85de5bb057ba300f2b9cfddeff67c41e8008315))
* **Ajika:** Lazy bodies ([#297](https://github.com/rust-glancer/rust-glancer/issues/297)) ([a8fce21](https://github.com/rust-glancer/rust-glancer/commit/a8fce2186701d854e96e30ceb748bebea8839455))
* **Ajika:** LSP integration ([#308](https://github.com/rust-glancer/rust-glancer/issues/308)) ([50dd51a](https://github.com/rust-glancer/rust-glancer/commit/50dd51a5c13b5e2c6486e3a5baace55c3b578711))
* **Ajika:** Remove batched and deferred indexing ([#291](https://github.com/rust-glancer/rust-glancer/issues/291)) ([f627c35](https://github.com/rust-glancer/rust-glancer/commit/f627c353ed01fab817f2005ab9e4d1bb7a08a934))
* **Ajika:** Separate associated items from the impl blocks ([#303](https://github.com/rust-glancer/rust-glancer/issues/303)) ([d67423d](https://github.com/rust-glancer/rust-glancer/commit/d67423d8a621b71b3babd2e36c82fc86ef0498b5))
* **Ajika:** Treat const/static initializers as body computations ([#301](https://github.com/rust-glancer/rust-glancer/issues/301)) ([bf4cf7c](https://github.com/rust-glancer/rust-glancer/commit/bf4cf7c1f6e9347afddea76197f31539fefe1465))
* Better coercions ([#318](https://github.com/rust-glancer/rust-glancer/issues/318)) ([ea158f9](https://github.com/rust-glancer/rust-glancer/commit/ea158f99e5b0bc99107a882c6b29f0618b4ed5c4))
* Better completions ([#319](https://github.com/rust-glancer/rust-glancer/issues/319)) ([65757a5](https://github.com/rust-glancer/rust-glancer/commit/65757a5b3b1a0c452ea7f57133e888f1c9dd1c16))
* Low-hanging analysis improvements ([#314](https://github.com/rust-glancer/rust-glancer/issues/314)) ([f0260c8](https://github.com/rust-glancer/rust-glancer/commit/f0260c8acae2a10c798a850d4bb341cf2ce699a4))
* Make all-offloadable the only supported policy for users ([#276](https://github.com/rust-glancer/rust-glancer/issues/276)) ([7bfe14e](https://github.com/rust-glancer/rust-glancer/commit/7bfe14eac1916f5883d670bfc0d519ca51246846))
* Minimal support for opening from subdirs ([#284](https://github.com/rust-glancer/rust-glancer/issues/284)) ([86dc5de](https://github.com/rust-glancer/rust-glancer/commit/86dc5dee760b4ab9aa8a4a5a234f4bc48d922d8f))
* Model async properly ([#312](https://github.com/rust-glancer/rust-glancer/issues/312)) ([120a989](https://github.com/rust-glancer/rust-glancer/commit/120a989661fe7a8c7cd95ccca4c11649c162fa99))
* Re-enable auto-imports in completions ([#313](https://github.com/rust-glancer/rust-glancer/issues/313)) ([1f892e7](https://github.com/rust-glancer/rust-glancer/commit/1f892e7971eb6c918ed5ac40b09ee401348b197d))
* Rework defmap 2 ([#283](https://github.com/rust-glancer/rust-glancer/issues/283)) ([b5ec01f](https://github.com/rust-glancer/rust-glancer/commit/b5ec01f086bea021ed828004ba318929d30d0714))
* Signature help ([#317](https://github.com/rust-glancer/rust-glancer/issues/317)) ([8ea9adb](https://github.com/rust-glancer/rust-glancer/commit/8ea9adb992067eb0da6e9d6caf4391c577175286))
* Simplify defmap ([#281](https://github.com/rust-glancer/rust-glancer/issues/281)) ([dbc7ca5](https://github.com/rust-glancer/rust-glancer/commit/dbc7ca59f2f70d26da09f9bc99904dcfb8bbbee8))
* Support rust 1.100 build dir layout ([#311](https://github.com/rust-glancer/rust-glancer/issues/311)) ([0140a90](https://github.com/rust-glancer/rust-glancer/commit/0140a904e97b9b33cda0c4ca5b8529c218225ea3))
* Use callable evidence ([#315](https://github.com/rust-glancer/rust-glancer/issues/315)) ([eb036d0](https://github.com/rust-glancer/rust-glancer/commit/eb036d0842e22198adbf7686c30f294e351ebd47))
* Use string_cache for interning ([#292](https://github.com/rust-glancer/rust-glancer/issues/292)) ([d321fb5](https://github.com/rust-glancer/rust-glancer/commit/d321fb51283d7147ae0e5387726a7ff0ddb4165c))


### Bug Fixes

* **Ajika:** Detour ([#299](https://github.com/rust-glancer/rust-glancer/issues/299)) ([255619c](https://github.com/rust-glancer/rust-glancer/commit/255619c35e2b6aa74b8f6497d4eb9debb5f735d5))
* Do not follow symlinked directories in the workspace watcher ([#272](https://github.com/rust-glancer/rust-glancer/issues/272)) ([aac870a](https://github.com/rust-glancer/rust-glancer/commit/aac870a118288ddb307f84d47b3cb9e78f96006f))
* Keep release asset filenames stable ([#316](https://github.com/rust-glancer/rust-glancer/issues/316)) ([fc0eb47](https://github.com/rust-glancer/rust-glancer/commit/fc0eb47f8b69d521a63ff0a7e28a6558df3fc6af))
* Stack overflow on cyclic dyn impl candidate ([#287](https://github.com/rust-glancer/rust-glancer/issues/287)) ([1a3faf4](https://github.com/rust-glancer/rust-glancer/commit/1a3faf41b29591bb2ed6cdbc7b2a9f093c16ad7d))


### Performance Improvements

* MOAR PERF ([#310](https://github.com/rust-glancer/rust-glancer/issues/310)) ([5061adc](https://github.com/rust-glancer/rust-glancer/commit/5061adc328464839d5d461d8d4f21134fd2f8e4a))
* Use name-based search for resolving hover links ([#309](https://github.com/rust-glancer/rust-glancer/issues/309)) ([f98c88c](https://github.com/rust-glancer/rust-glancer/commit/f98c88c58b65e90c4f9d12bf38771d2c7a2641c4))

## [0.4.0](https://github.com/rust-glancer/rust-glancer/compare/v0.3.0...v0.4.0) (2026-10-21)


### Features

* Auto traits and impl polarity ([#252](https://github.com/rust-glancer/rust-glancer/issues/252)) ([1dcb1fd](https://github.com/rust-glancer/rust-glancer/commit/1dcb1fd1cc8da4473f5fadd8393a28921c481ff1))
* Basic const evaluation ([#244](https://github.com/rust-glancer/rust-glancer/issues/244)) ([e7ea5be](https://github.com/rust-glancer/rust-glancer/commit/e7ea5be36f2941f30ae507bf7d10f29b0f485025))
* Basic dyn trait support ([#249](https://github.com/rust-glancer/rust-glancer/issues/249)) ([bac823d](https://github.com/rust-glancer/rust-glancer/commit/bac823d748d274ea0821f55447257b365b77d154))
* Better const evaluation ([#264](https://github.com/rust-glancer/rust-glancer/issues/264)) ([0c871d6](https://github.com/rust-glancer/rust-glancer/commit/0c871d65f3603012b40c77b5b729099023739141))
* Better inlay hints ([#260](https://github.com/rust-glancer/rust-glancer/issues/260)) ([3c19eb7](https://github.com/rust-glancer/rust-glancer/commit/3c19eb78f37c70e7b5c3515f65d99ff1de9d1460))
* Resolve/normalize Self-based associated types in signatures/parameters ([#263](https://github.com/rust-glancer/rust-glancer/issues/263)) ([4a4053d](https://github.com/rust-glancer/rust-glancer/commit/4a4053df44822cbf06704edbe51abf1a6c782c37))
* Richer support for arguments in macro expansions ([#266](https://github.com/rust-glancer/rust-glancer/issues/266)) ([9d68b1e](https://github.com/rust-glancer/rust-glancer/commit/9d68b1efb47703f56767643610802e4591a55188))
* Support tuple/slice pattern destructing ([#262](https://github.com/rust-glancer/rust-glancer/issues/262)) ([0d6b1a4](https://github.com/rust-glancer/rust-glancer/commit/0d6b1a40ec6463c47c0a5afe548af66494b343e5))


### Bug Fixes

* Better definition spans ([#259](https://github.com/rust-glancer/rust-glancer/issues/259)) ([e5e544c](https://github.com/rust-glancer/rust-glancer/commit/e5e544c52d0dd03af386c2844504c1724e9f32f7))
* Fix closure explicit return types ([#250](https://github.com/rust-glancer/rust-glancer/issues/250)) ([c1457eb](https://github.com/rust-glancer/rust-glancer/commit/c1457ebcc18d673eb070a627deb175e9a049ff4a))
* Fix inlay hints flickering ([#246](https://github.com/rust-glancer/rust-glancer/issues/246)) ([4877fcb](https://github.com/rust-glancer/rust-glancer/commit/4877fcb8c5d14bcc2046c94c98e3b6f40833201c))
* Fix memory leak via mimalloc ([#269](https://github.com/rust-glancer/rust-glancer/issues/269)) ([92519d7](https://github.com/rust-glancer/rust-glancer/commit/92519d74b0bf41f2592168b4f1ad0e6de8f72b81))
* Preserve references to unknown ([#248](https://github.com/rust-glancer/rust-glancer/issues/248)) ([688baa6](https://github.com/rust-glancer/rust-glancer/commit/688baa67a30046a93ade61e4478ce26be5c68da0))
* Use a single target when file belongs to multiple targets ([#267](https://github.com/rust-glancer/rust-glancer/issues/267)) ([341c105](https://github.com/rust-glancer/rust-glancer/commit/341c105b6aea0a23f4f76dc0bf5f313f444b0a9c))


### Performance Improvements

* Faster faster references ([#254](https://github.com/rust-glancer/rust-glancer/issues/254)) ([c6a0f67](https://github.com/rust-glancer/rust-glancer/commit/c6a0f678394ea69ab19834dc27e731163e037eeb))
* Optimize item lookup query creation ([#253](https://github.com/rust-glancer/rust-glancer/issues/253)) ([626dd41](https://github.com/rust-glancer/rust-glancer/commit/626dd418ac1734d3b46716bc250dec6515df2dd4))


### Miscellaneous Chores

* release 0.4.0 ([2a998d2](https://github.com/rust-glancer/rust-glancer/commit/2a998d2200986242cfdf3acb85136862337edec5))

## [0.3.0](https://github.com/rust-glancer/rust-glancer/compare/v0.2.0...v0.3.0) (2026-10-01)


### Features

* Better hover signature formatting ([#230](https://github.com/rust-glancer/rust-glancer/issues/230)) ([ce24787](https://github.com/rust-glancer/rust-glancer/commit/ce24787202e2b78ce21e67efa7d455d0e827711e))
* Better purging after indexing ([#224](https://github.com/rust-glancer/rust-glancer/issues/224)) ([336f2c4](https://github.com/rust-glancer/rust-glancer/commit/336f2c40f43f1f8ffe23477f9d952925c6b0916d))
* Change type inference scheduler from fixed loop to recursive approach ([#225](https://github.com/rust-glancer/rust-glancer/issues/225)) ([6e351fb](https://github.com/rust-glancer/rust-glancer/commit/6e351fb7a1ad947e36251605188c9eea6098d04d))
* Default derive macros ([#234](https://github.com/rust-glancer/rust-glancer/issues/234)) ([9ec2f00](https://github.com/rust-glancer/rust-glancer/commit/9ec2f00b4daf0014517e20de9cd4c3ee8439e57c))
* goto module definition file on goto definition on module ([#231](https://github.com/rust-glancer/rust-glancer/issues/231)) ([779f280](https://github.com/rust-glancer/rust-glancer/commit/779f2801ed1ee52ac58265245cb08b36e2f61db9))
* Implement folding ([#211](https://github.com/rust-glancer/rust-glancer/issues/211)) ([8aed80e](https://github.com/rust-glancer/rust-glancer/commit/8aed80e34f2d0cf0585436f74e68a77f317b6217))
* Support rust syntax and rustdoc links in doccomments and hovers ([#214](https://github.com/rust-glancer/rust-glancer/issues/214)) ([e9aa584](https://github.com/rust-glancer/rust-glancer/commit/e9aa584bb93e51bb2da364b50be2e2d764830c2d))
* Support Try, Range, Index operations ([#239](https://github.com/rust-glancer/rust-glancer/issues/239)) ([0845193](https://github.com/rust-glancer/rust-glancer/commit/0845193ab700e58e1598f60f15e655fa715834b8))
* Update rust-analyzer dep ([#227](https://github.com/rust-glancer/rust-glancer/issues/227)) ([c379461](https://github.com/rust-glancer/rust-glancer/commit/c379461da0468a70e44d2217413f1b6d4b5fa528))
* Yay new trait solver ([#229](https://github.com/rust-glancer/rust-glancer/issues/229)) ([2b6194b](https://github.com/rust-glancer/rust-glancer/commit/2b6194bd6ecd4998dda395400c5230c30ce14c0b))


### Bug Fixes

* Fallback on rootUri if no workspace in initial ([#207](https://github.com/rust-glancer/rust-glancer/issues/207)) ([63e67a0](https://github.com/rust-glancer/rust-glancer/commit/63e67a07becf7310ebe8dea7ba9b1d105f9ed4ae))
* Find implementations should find all implementations ([#240](https://github.com/rust-glancer/rust-glancer/issues/240)) ([1e5850b](https://github.com/rust-glancer/rust-glancer/commit/1e5850bf201930dbef7637a6030cf8f27a3bd2f0))
* Fix break label block inference ([#237](https://github.com/rust-glancer/rust-glancer/issues/237)) ([1d42bd7](https://github.com/rust-glancer/rust-glancer/commit/1d42bd7c3150befe641d64b9dc34198c4e792575))
* Goto implementation follows relevant trait impl ([#236](https://github.com/rust-glancer/rust-glancer/issues/236)) ([f4240ff](https://github.com/rust-glancer/rust-glancer/commit/f4240ff7c6e109bd2e752e12100089369c48ee37))
* Tech debt management ([#213](https://github.com/rust-glancer/rust-glancer/issues/213)) ([c81f62d](https://github.com/rust-glancer/rust-glancer/commit/c81f62d04188e67b5c97976723208b1ad6dadb8c))


### Performance Improvements

* Add dist profile for releases ([#223](https://github.com/rust-glancer/rust-glancer/issues/223)) ([7b72615](https://github.com/rust-glancer/rust-glancer/commit/7b726157454bf2321407d1512ebcaa2ca14e1998))


### Miscellaneous Chores

* prepare 0.3.0 release ([aba2feb](https://github.com/rust-glancer/rust-glancer/commit/aba2feb843564e55dbc2486e3392199113962d73))

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
