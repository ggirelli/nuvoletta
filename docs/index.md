# nuvoletta

*Private thoughts. Safely kept.*

nuvoletta is an encrypted notes app written in Rust with Dioxus. Your notes are encrypted before they leave your device, so only you can read them.

!!! warning "Pre-alpha"
    nuvoletta is in pre-alpha. It is **not for sensitive data**, and it has **not been audited**. Expect breaking changes.

## Principles

- Notes are encrypted at rest and in transit; the server never sees plaintext.
- The design is written down first: threat model, formats and protocols are documented.
- Decisions are recorded as ADRs.

## Where to go next

- [Plan](plan.md)
- [Threat model](THREAT_MODEL.md), [Crypto format](CRYPTO_FORMAT.md), [Sync protocol](SYNC_PROTOCOL.md)
- [Development](development.md)
- [Source code](https://github.com/ggirelli/nuvoletta)
