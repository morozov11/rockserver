# Vendored gRPC contract schemas

Protobuf schemas for the neural-service gRPC contract, compiled at build time by
`build.rs` into tonic/prost types and re-exported through `src/grpc.rs`.

Copied verbatim from the `rock-llm-service` repository (`C:\repos\rockmodules`),
commit `6284631a913736b01e14c82f6e7e889a3deafc40` (stage E1). The service
repository owns the contract (`proto/` + `docs/contract.md`); update the copies
here only together with a service release, recording the new commit and hashes.

| File | Package | SHA-256 |
| --- | --- | --- |
| `embed/v1/embed.proto` | `embed.v1` | `6f5603bb8741d40ef5639eeab3891da5e58c97868c370127abf825f872f8d0c9` |
| `google/rpc/error_details.proto` | `google.rpc` | `66ef1472fc4def13c36dc7e74f1ceb92636be56da82de389499499c0991c432c` |

`google/protobuf/any.proto` is not vendored: protoc resolves well-known types
internally and prost maps them onto `prost_types`.
