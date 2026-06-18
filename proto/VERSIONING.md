# Omnimodem gRPC API — Stability & Versioning Policy

Third-party frontends are a primary goal, so the wire contract is versioned
from day one.

## Package versioning

- The proto package carries a major version: `omnimodem.v1`.
- The major version follows semantic versioning at the API level.

## Additive-only within a major

Within a major version (`v1`), every change MUST be backward compatible:

- New messages, fields, RPCs, and enum values may be added.
- Existing field tags are NEVER reused, renumbered, or repurposed.
- Fields are NEVER removed; deprecate them (`// deprecated`) and `reserved`
  the tag if they must go.
- Enum value numbers are stable; the zero value stays `*_UNSPECIFIED` where one
  is defined.
- RPC method names and their request/response message types are stable.

## Breaking changes

A breaking change requires a new package (`omnimodem.v2`) served alongside `v1`
during a deprecation window. The major version constant
(`proto::API_VERSION_MAJOR`) is bumped in lockstep.

## Review gate

Any PR touching `proto/omnimodem.proto` must confirm in its description that the
change is additive within the current major, or that it introduces a new major.
