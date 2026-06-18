# Architecture Decision Records (ADRs)

This directory records **why** Skyfire is built the way it is. One file per
decision, numbered, immutable once `Accepted` — to change a decision, write a new
ADR that supersedes the old one (and mark the old one `Superseded by NNNN`).

## Index

| # | Title | Status | Date |
|---|---|---|---|
| [0001](0001-browser-and-platform-support.md) | Browser & platform support, codec-gating policy | Accepted | 2026-06-18 |

## Format

Each ADR has: **Status** (Proposed / Accepted / Superseded), **Context** (forces
at play), **Decision** (what we chose), **Consequences** (what this commits us to).
Keep them short. Cite specs/APIs where a claim is technical.

## Rules

- Never edit an `Accepted` ADR's decision — supersede it with a new one.
- Every new decision lands here **and** updates this index table in the same change.
- Objectives & roadmap live in [`../OBJECTIVES.md`](../OBJECTIVES.md), not here.
