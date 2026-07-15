# Changelog

All notable changes to open-media are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.0.2

### Added

- AniList airing and seasonal catalogs on the TUI Discover home
- Dynamic episode discovery from release titles when metadata episode lists are incomplete

### Fixed

- Multi-season nyaa queries now respect the requested season (e.g. Mushoku S3)
- Nyaa rate-limit backoff, response caching, and mock-server e2e stability
- Catalog failures surface as errors instead of silent empty results
- Nix `cargoLock.outputHashes` for open-subtitle git deps

## 0.0.1

Initial public line of the dual-use open-media client:

- Terminal media client: metadata search, local library, mpv/vlc control
- Optional torrent-index and P2P adapters **off by default** (see `docs/LEGAL.md`)
- Optional debrid backends with user-supplied tokens
- AniList/MAL tracking, resume, AniSkip, Discord presence
- Distributed via GitHub Releases and Nix — workspace crates are not published to crates.io
