# Architecture outline

## Scope and boundaries

RockServer owns remote station discovery: accepting a natural-language query, interpreting it, searching a catalog, ranking candidates, and returning stream metadata. It does not own the Windows UI or audio playback. Those remain in RockCast, which also retains a local catalog fallback.

## Planned request flow

1. RockCast sends a request to the versioned search endpoint.
2. The HTTP layer validates the DTO and assigns a request ID.
3. A query-parser provider converts free text into structured filters and intent.
4. The search domain applies hard filters to catalog candidates.
5. Ranking combines deterministic metadata signals with semantic similarity when available.
6. The HTTP layer maps ranked domain results to the contract DTO.
7. RockCast selects a result and passes its stream URL to the existing playback controller.

Provider failures must degrade predictably. Query parsing and embeddings will live behind traits, allowing deterministic fake implementations in unit tests. The request path must not depend on crawling Radio Browser or probing streams; importing and health checking are background concerns.

## Planned modules

- **API:** Axum routing, DTO validation, error mapping, request IDs, and OpenAPI alignment.
- **Domain:** queries, filters, station candidates, scoring, and stable ranking.
- **Persistence:** catalog access, PostgreSQL migrations, pgvector queries, and imports.
- **Providers:** replaceable query-parser and embedding implementations.
- **Operations:** tracing, health checks, metrics, rate limiting, shutdown, and deployment configuration.

Dependencies should point inward toward domain concepts. HTTP, database, and provider-specific types must not leak into the core ranking model.
