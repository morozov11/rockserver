-- Full-text search index over the pre-built searchable_text column.
-- Uses 'simple' config: no stemming, language-neutral, preserves exact tokens.
ALTER TABLE stations ADD COLUMN IF NOT EXISTS searchable_tsv tsvector;

UPDATE stations SET searchable_tsv = to_tsvector('simple', searchable_text)
WHERE searchable_tsv IS NULL;

ALTER TABLE stations ALTER COLUMN searchable_tsv SET NOT NULL;

CREATE INDEX IF NOT EXISTS stations_searchable_tsv_idx
    ON stations USING gin (searchable_tsv);

CREATE OR REPLACE FUNCTION stations_searchable_tsv_trigger() RETURNS trigger AS $$
BEGIN
  NEW.searchable_tsv := to_tsvector('simple', NEW.searchable_text);
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER stations_searchable_tsv_update
  BEFORE INSERT OR UPDATE OF searchable_text ON stations
  FOR EACH ROW EXECUTE FUNCTION stations_searchable_tsv_trigger();
