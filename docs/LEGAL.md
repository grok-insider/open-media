# Legal notice — dual-use client

**This is not legal advice.** Copyright and related rules differ by country and
change over time. For a definitive answer about your situation, consult a lawyer
licensed where you live.

## What open-media is

open-media is **local client software**. It:

- discovers **metadata** (titles, IDs, artwork) via third-party APIs you choose
  to use (e.g. TMDB, Cinemeta, AniList);
- can optionally query **third-party indexes / addons** and **debrid APIs** using
  **your** credentials;
- can optionally stream via a **local** BitTorrent engine (librqbit) bound to
  `127.0.0.1`;
- plays media in **your** player (mpv/vlc);
- does **not** host movies, series, anime files, magnet archives, or torrent
  indexes on project infrastructure.

The MIT license covers the **software source code**, not any third-party media.

## What open-media is not

- A license or authorization to copy, stream, or share copyrighted works.
- A content platform, CDN, or public rebroadcast server.
- A DRM-circumvention tool (official streaming DRM integrations and cracks are
  out of scope; see `CONTRIBUTING.md`).
- A guarantee that any particular use is lawful in your jurisdiction.

## Your responsibility

You are solely responsible for ensuring that **your use** of open-media complies
with:

1. **Copyright and related rights** under the laws of your jurisdiction
   (including rules on reproduction and making works available to the public);
2. **Civil and criminal** rules that may apply to unauthorized use of protected
   works;
3. **Terms of service** of every third party you connect (Torrentio, nyaa.si,
   Real-Debrid, TorBox, TMDB, AniList, MAL, subtitle providers, etc.).

Optional source adapters (Torrentio, nyaa) and local P2P are **off by default**.
Enable them only if you understand and accept that responsibility. See the
README section *Optional source adapters*.

## Dual-use notes (product design)

These points inform defaults and documentation. They are **not** a complete
statement of any country’s law.

| Topic | Practical takeaway |
|-------|--------------------|
| **Dual-use software** | Publishing a local client (like a browser or BitTorrent app) is generally treated differently from operating a commercial unlicensed streaming site or hosting infringing files. open-media aims to stay a **tool**, not a content host. |
| **Unauthorized catalogs** | Using any tool to access protected works **without authorization** can engage exclusive rights. Do **not** assume “personal use = always legal.” Private-copy or similar exceptions, where they exist, are typically **narrow** and are not a general free pass for unlicensed P2P or streams. |
| **P2P upload / seeding** | Participating in a BitTorrent swarm often **uploads** pieces to other peers. That can amount to making a work available to others—often a higher-risk act than local playback alone. |
| **Debrid services** | A debrid account may avoid *local* swarm traffic. It does **not** grant you a copyright license to the underlying work. You must still comply with copyright law and the debrid provider’s terms. |
| **Commercial facilitation** | Projects that **host** indexes, profit from organizing access to infringing files, or market themselves as free commercial streaming face a different risk profile than a dual-use local client with opt-in adapters. |

## Features and risk surface

| Path | What happens | Notes |
|------|----------------|-------|
| Metadata only | Catalog search via APIs | Generally subject only to those APIs’ ToS |
| Debrid playback | Your token → remote CDN URL → player | No local BitTorrent by design when resolution succeeds via debrid; still not a media license |
| P2P playback | Local librqbit session; HTTP only on `127.0.0.1` | May download **and upload** pieces; temp data under the system temp dir (`open-media-p2p`); off unless `streaming.allow_p2p = true` |
| Source discovery | Queries public addons/indexes | Opt-in (`providers.torrentio`, `providers.nyaa_direct`) |

## Project commitments

Contributions and project operations must not:

- bundle API tokens, shared debrid accounts, or magnet/torrent corpora;
- add DRM circumvention or cracks of official clients;
- scrape behind authentication walls;
- run a project-operated public stream proxy or CDN for user media;
- document “how to infringe” as a feature.

See `CONTRIBUTING.md` → Scope & legality.

## Telemetry

Anonymous install pings (version, OS, arch, random install id) never include
titles, queries, or watch history. See the README telemetry section.

## Contact / enforcement

open-media does not host user video content. Rights-holder notices about
**third-party** indexes or debrid services should go to those operators. Issues
about **this repository’s code** can be filed on the project’s GitHub tracker.

---

*Last updated for the dual-use / opt-in sources posture. Review by counsel is
recommended before relying on this document as official corporate policy.*
