-- Active lifecycle metadata for provider-scoped canonical catalog identities. This is deliberately
-- separate from `stations`: retired rows retain operational/audit history while replacement
-- semantics remain available even after the station is no longer searchable.
CREATE TABLE catalog_tombstones (
    source text NOT NULL,
    retired_station_id varchar(128) NOT NULL,
    reason text NOT NULL CHECK (reason IN ('removed', 'merged', 'split')),
    replacement_ids text[] NOT NULL DEFAULT '{}',
    catalog_version text NOT NULL,
    activated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (source, retired_station_id),
    CHECK (
        (reason = 'removed' AND cardinality(replacement_ids) = 0)
        OR (reason = 'merged' AND cardinality(replacement_ids) = 1)
        OR (reason = 'split' AND cardinality(replacement_ids) >= 2)
    )
);

CREATE INDEX catalog_tombstones_source_version_idx
    ON catalog_tombstones (source, catalog_version);
