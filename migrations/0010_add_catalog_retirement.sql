-- Provider-scoped soft retirement lets a released baseline withdraw records without touching
-- independent provider ownership or deleting operational history.
ALTER TABLE stations ADD COLUMN retired_at timestamptz;
ALTER TABLE station_streams ADD COLUMN retired_at timestamptz;

CREATE INDEX stations_active_source_idx ON stations (source, retired_at);
CREATE INDEX station_streams_active_source_idx ON station_streams (source, retired_at);

-- These rows were the temporary hard-coded development seed. The pinned `rockcatalog` release
-- is activated by the repository bootstrap after migrations complete.
DELETE FROM stations WHERE source = 'builtin';
