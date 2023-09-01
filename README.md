**IMPORTANT**: This library is in beta, and should not be used in production software.

This is a GPU-specific optimization experiment version of halo2 that tries to get the most out of GPU for halo2 with KZG commitment.

## Minimum Supported Rust Version

Requires Rust **1.66** or higher.
Require Cuda compilation tools, release >= 12.0 or 12.2

## Controlling parallelism

`halo2` currently uses [rayon](https://github.com/rayon-rs/rayon) for parallel computation.
The `RAYON_NUM_THREADS` environment variable can be used to set the number of threads.
`multi-gpu` is supported with a configurable depth of cache size.

## License

Copyright 2020-2021 The Electric Coin Company.

You may use this package under the Bootstrap Open Source Licence, version 1.0,
or at your option, any later version. See the file [`COPYING`](COPYING) for
more details, and [`LICENSE-BOSL`](LICENSE-BOSL) for the terms of the Bootstrap
Open Source Licence, version 1.0.

The purpose of the BOSL is to allow commercial improvements to the package
while ensuring that all improvements are open source. See
[here](https://electriccoin.co/blog/introducing-tgppl-a-radically-new-type-of-open-source-license/)
for why the BOSL exists.
