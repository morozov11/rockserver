-- Speed up substring name matching (LIKE '%term%') used by station search.
-- Without pg_trgm, every substring query causes a sequential scan.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS stations_name_lower_trgm_idx
    ON public.stations
    USING gin ((lower(name)) gin_trgm_ops);

