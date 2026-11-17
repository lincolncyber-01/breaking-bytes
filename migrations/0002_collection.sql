-- Expansão compatível: a versão anterior ignora os novos campos.
ALTER TABLE sources ADD COLUMN etag text;
ALTER TABLE sources ADD COLUMN last_modified text;
ALTER TABLE sources ADD COLUMN last_polled_at timestamptz;
ALTER TABLE sources ADD COLUMN last_success_at timestamptz;
ALTER TABLE sources ADD COLUMN failures int NOT NULL DEFAULT 0 CHECK (failures >= 0);
ALTER TABLE sources ADD COLUMN next_poll_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE sources ADD COLUMN last_error_code text;

ALTER TABLE articles ADD COLUMN entities text[] NOT NULL DEFAULT '{}';
ALTER TABLE articles ADD COLUMN facts text[] NOT NULL DEFAULT '{}';
ALTER TABLE articles ADD COLUMN tokens text[] NOT NULL DEFAULT '{}';
ALTER TABLE events ADD COLUMN parent_event_id bigint REFERENCES events(id);
ALTER TABLE events ADD COLUMN primary_article_id bigint REFERENCES articles(id);

CREATE TABLE article_sightings (
    article_id bigint NOT NULL REFERENCES articles(id),
    source_id int NOT NULL REFERENCES sources(id),
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (article_id,source_id)
);
CREATE INDEX articles_entities_idx ON articles USING gin (entities);
CREATE INDEX jobs_kind_ready_idx ON jobs (kind,run_at,id) WHERE done_at IS NULL AND dead_at IS NULL;
CREATE INDEX event_articles_article_idx ON event_articles(article_id);
CREATE INDEX events_created_idx ON events(created_at DESC);
