![nuvoletta](assets/brand/10-docs-hero-light.svg#only-light)
![nuvoletta](assets/brand/11-docs-hero-dark.svg#only-dark)

# nuvoletta

*Private thoughts. Safely kept.*

nuvoletta is an encrypted notes app written in Rust with Dioxus. Your notes are encrypted before they leave your device, so only you can read them.

!!! warning "Pre-release, not audited, fully vibe coded"
    - nuvoletta is a **pre-release**. It is **not for sensitive data** yet, and it has **not been audited**. Keep backups and expect breaking changes.
    - **Fully vibe coded:** an AI assistant (Claude) writes all of its code, tests, formats and docs; the maintainer directs and reviews the work. Read the [disclaimer](disclaimer.md).
    - **Free and open source** under the [MIT license](https://github.com/ggirelli/nuvoletta/blob/main/LICENSE), provided as is, **without warranty**.

## Principles

- Notes are encrypted at rest and in transit; the server never sees plaintext.
- The design is written down first: threat model, formats and protocols are documented.
- Decisions are recorded as ADRs.

## Where to go next

- [Plan](plan.md)
- [Threat model](THREAT_MODEL.md), [Crypto format](CRYPTO_FORMAT.md), [Sync protocol](SYNC_PROTOCOL.md)
- [User guide](user/index.md)
- [Development](development.md)
- [Source code](https://github.com/ggirelli/nuvoletta)
