# ADR 0001: Record architecture decisions

Status: Proposed (Phase 0, not reviewed)
Date: 2026-11-12

## Context

Nuvoletta has security-sensitive and hard-to-reverse decisions (encrypted formats, sync semantics, framework choice). The project is solo-developed with AI-assisted review, so the reasoning must be written down to be reviewable later and by others.

## Decision

We record significant architecture decisions as Architecture Decision Records in `docs/ADR/`, using the lightweight format described by Michael Nygard.

- File name: `NNNN-short-title.md`, numbered sequentially, never renumbered.
- Sections: Title, Status (Proposed, Accepted, Deprecated, Superseded by NNNN), Date, Context, Decision, Consequences (and Alternatives considered where useful).
- An ADR is required for: any change to the byte-level formats, cryptographic primitives or parameters, sync protocol semantics, a new persistent dependency class (for example a database), platform or framework changes, and anything that weakens a documented security property.
- Accepted ADRs are not edited except for status; changes are made by a new ADR that supersedes the old one.
- ADRs and the design documents (`THREAT_MODEL.md`, `CRYPTO_FORMAT.md`, `SYNC_PROTOCOL.md`) must agree; a PR that changes one updates the other.

## Consequences

- Small overhead per decision; much cheaper than reconstructing intent later.
- Phase 0 exit review can check the ADR list for unresolved or undocumented choices.
