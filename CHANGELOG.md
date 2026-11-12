# Changelog

Every release of beam, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and beam uses
[Semantic Versioning](https://semver.org/): while the version starts with 0,
a new middle number (0.2.0) can change behaviour, and a new last number
(0.1.1) only fixes. What is planned is in the [roadmap](docs/roadmap.md).

## [Unreleased]

## [0.1.0] - 2026-11-12

The first public release: everything built so far. What beam does is
described in the [README](README.md), and which feature is in which front end
in [docs/parity.md](docs/parity.md). The decisions behind it are ADR-0001 to
ADR-0058 in [docs/decisions.md](docs/decisions.md).

The first downloads: a setup wizard and a zip for Windows, each with its
SHA-256 in `SHA256SUMS.txt`, and `THIRD-PARTY.html` with the licence of
every crate built into the programs. For Arch Linux, a `PKGBUILD`
(`packaging/arch`) builds the `beam` and `beam-gui` packages; they go to the
AUR once its account registration, closed for now, reopens.
