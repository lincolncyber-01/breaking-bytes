ALTER TABLE events ADD COLUMN publication_kind text NOT NULL DEFAULT 'new' CHECK(publication_kind IN('new','update','correction'));
UPDATE events SET publication_kind='update' WHERE parent_event_id IS NOT NULL;
ALTER TABLE publications ADD COLUMN editorial_snapshot jsonb;

CREATE TABLE publication_watches (
    publication_id bigint PRIMARY KEY REFERENCES publications(id),
    event_id bigint NOT NULL REFERENCES events(id),
    dry_run bool NOT NULL,
    baseline_draft jsonb NOT NULL,
    version bigint NOT NULL DEFAULT 0 CHECK(version>=0),
    fingerprint bytea CHECK(octet_length(fingerprint)=32),
    expires_at timestamptz NOT NULL,
    superseded_by bigint REFERENCES publications(id),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE publication_source_snapshots (
    publication_id bigint NOT NULL REFERENCES publication_watches(publication_id),
    article_id bigint NOT NULL REFERENCES articles(id),
    source_id int NOT NULL REFERENCES sources(id),
    baseline jsonb NOT NULL,
    current_snapshot jsonb,
    content_hash text CHECK(content_hash IS NULL OR content_hash ~ '^[0-9a-f]{64}$'),
    notice text CHECK(notice IS NULL OR char_length(notice)<=500),
    fetched_at timestamptz,
    attempted_at timestamptz,
    failures int NOT NULL DEFAULT 0 CHECK(failures BETWEEN 0 AND 32),
    last_error_code text CHECK(last_error_code IS NULL OR last_error_code IN('http','parse','limits','kind','worker','unavailable','identity_changed')),
    PRIMARY KEY(publication_id,article_id,source_id)
);
CREATE TABLE publication_reviews (
    id bigserial PRIMARY KEY,
    publication_id bigint NOT NULL REFERENCES publication_watches(publication_id),
    version bigint NOT NULL CHECK(version>0),
    context jsonb NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK(status IN('pending','applied','ignored','uncertain','failed','superseded')),
    decision jsonb,
    candidate_event_id bigint REFERENCES events(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    decided_at timestamptz,
    UNIQUE(publication_id,version)
);
ALTER TABLE events ADD COLUMN review_id bigint REFERENCES publication_reviews(id);
ALTER TABLE events ADD CONSTRAINT correction_requires_review CHECK(publication_kind<>'correction' OR review_id IS NOT NULL);

CREATE FUNCTION default_publication_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.parent_event_id IS NOT NULL AND NEW.publication_kind='new' AND NEW.review_id IS NULL THEN
        NEW.publication_kind:='update';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER events_publication_kind BEFORE INSERT ON events FOR EACH ROW EXECUTE FUNCTION default_publication_kind();

-- Confirmação de mudança não rejuvenesce a data da matéria; a prova é a reconsulta.
CREATE FUNCTION revision_evidence_fresh(target bigint,age_hours int,simulation bool) RETURNS bool LANGUAGE sql STABLE AS $$
    SELECT EXISTS(
        SELECT 1 FROM events e JOIN publication_reviews r ON r.id=e.review_id
        JOIN publication_watches w ON w.publication_id=r.publication_id
        WHERE e.id=target AND r.status='applied' AND r.version=w.version
          AND w.superseded_by IS NULL AND w.expires_at>now() AND w.dry_run=simulation
          AND e.parent_event_id=w.event_id
          AND e.publication_kind=CASE WHEN r.decision->>'decision' IN('correction','retraction') THEN 'correction' ELSE 'update' END
          AND jsonb_array_length(r.context->'changes') BETWEEN 1 AND 4
          AND NOT EXISTS(
              SELECT 1 FROM jsonb_array_elements(r.context->'changes') c
              LEFT JOIN publication_source_snapshots p ON p.publication_id=w.publication_id
                  AND p.article_id=(c->'after'->>'article_id')::bigint AND p.source_id=(c->'after'->>'source_id')::int
              LEFT JOIN sources s ON s.id=p.source_id
              WHERE p.content_hash IS DISTINCT FROM c->>'after_hash'
                 OR p.last_error_code IS NOT NULL OR p.fetched_at IS NULL OR p.fetched_at>now()
                 OR p.fetched_at<now()-make_interval(hours=>LEAST(age_hours,(r.context->>'max_age_hours')::int))
                 OR NOT COALESCE(s.enabled,false)
                 OR s.url IS DISTINCT FROM c->'before'->>'source_url'
                 OR s.name IS DISTINCT FROM c->'before'->>'source_name'
                 OR s.kind IS DISTINCT FROM c->'before'->>'kind'
                 OR s.layer::text IS DISTINCT FROM c->'before'->>'layer'
                 OR s.weight IS DISTINCT FROM (c->'before'->>'weight')::smallint
                 OR s.origin_group IS DISTINCT FROM c->'before'->>'origin_group'
          )
    );
$$;
CREATE FUNCTION publication_evidence_fresh(target bigint,age_hours int,simulation bool) RETURNS bool LANGUAGE sql STABLE AS $$
    SELECT CASE WHEN e.review_id IS NOT NULL THEN revision_evidence_fresh(target,age_hours,simulation)
        ELSE EXISTS(SELECT 1 FROM event_articles WHERE event_id=target)
          AND NOT EXISTS(SELECT 1 FROM event_articles ea JOIN articles a ON a.id=ea.article_id
              WHERE ea.event_id=target AND (a.published_at IS NULL OR a.published_at>now() OR a.published_at<now()-make_interval(hours=>age_hours)))
        END FROM events e WHERE e.id=target;
$$;
CREATE INDEX publication_reviews_status ON publication_reviews(status,created_at);
