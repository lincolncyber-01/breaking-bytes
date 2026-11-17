CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE sources (
    id serial PRIMARY KEY,
    name text UNIQUE NOT NULL,
    layer char(1) NOT NULL CHECK (layer IN ('A','B','C','D')),
    kind text NOT NULL,
    url text NOT NULL,
    weight smallint NOT NULL CHECK (weight BETWEEN 1 AND 5),
    interval_s int NOT NULL CHECK (interval_s >= 15),
    enabled bool NOT NULL DEFAULT true
);

CREATE TABLE articles (
    id bigserial PRIMARY KEY,
    source_id int NOT NULL REFERENCES sources(id),
    url_canonical text UNIQUE NOT NULL,
    title text NOT NULL,
    title_hash bytea NOT NULL,
    body_excerpt text,
    image_url text,
    published_at timestamptz,
    detected_at timestamptz NOT NULL DEFAULT now(),
    embedding vector(384)
);

CREATE TABLE events (
    id bigserial PRIMARY KEY,
    state text NOT NULL DEFAULT 'DETECTED' CHECK (state IN (
        'DETECTED','CLUSTERED','VERIFIED','DRAFTED','CHECKED','RENDERED',
        'QUEUED','PUBLISHING','PUBLISHED','REJECTED','SUPERSEDED','FAILED','NEEDS_CORRECTION'
    )),
    reason text,
    level smallint CHECK (level BETWEEN 1 AND 3),
    score real CHECK (score BETWEEN 0 AND 1),
    entities jsonb,
    title_pt text,
    summary_pt text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (state NOT IN ('REJECTED','SUPERSEDED','FAILED','NEEDS_CORRECTION')
        OR length(trim(reason)) > 0 AND reason IS NOT NULL)
);

CREATE TABLE event_articles (
    event_id bigint NOT NULL REFERENCES events(id),
    article_id bigint NOT NULL REFERENCES articles(id),
    PRIMARY KEY (event_id, article_id)
);

CREATE TABLE publications (
    id bigserial PRIMARY KEY,
    event_id bigint NOT NULL REFERENCES events(id),
    kind text NOT NULL CHECK (kind IN ('new','update','correction')),
    seq int NOT NULL DEFAULT 0 CHECK (seq >= 0),
    status text NOT NULL CHECK (status IN (
        'creating','container_ready','publishing','uncertain','published','dry_run','failed'
    )),
    container_id text UNIQUE,
    media_id text UNIQUE,
    image_key text,
    dry_run bool NOT NULL DEFAULT true,
    request_started_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    published_at timestamptz,
    UNIQUE (event_id, kind, seq),
    CHECK (status <> 'published' OR (media_id IS NOT NULL AND published_at IS NOT NULL AND NOT dry_run)),
    CHECK (status <> 'dry_run' OR dry_run)
);

CREATE TABLE jobs (
    id bigserial PRIMARY KEY,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    dedupe_key text NOT NULL UNIQUE,
    run_at timestamptz NOT NULL DEFAULT now(),
    attempts int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    locked_at timestamptz,
    lease_until timestamptz,
    lease_token uuid,
    done_at timestamptz,
    dead_at timestamptz,
    last_error_code text,
    CHECK ((lease_until IS NULL) = (lease_token IS NULL))
);

CREATE TABLE audit_log (
    id bigserial PRIMARY KEY,
    event_id bigint REFERENCES events(id),
    step text NOT NULL,
    detail jsonb,
    at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE blocklist (
    id serial PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('source','term','topic')),
    value text NOT NULL,
    UNIQUE (kind, value)
);

CREATE TABLE pipeline_control (
    id bool PRIMARY KEY DEFAULT true CHECK (id),
    paused bool NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO pipeline_control (id) VALUES (true);

CREATE INDEX articles_detected_idx ON articles (detected_at DESC);
CREATE INDEX articles_title_hash_idx ON articles (title_hash);
CREATE INDEX articles_embedding_idx ON articles USING hnsw (embedding vector_cosine_ops);
CREATE INDEX jobs_ready_idx ON jobs (run_at, id) WHERE done_at IS NULL AND dead_at IS NULL;
CREATE INDEX publications_reconcile_idx ON publications (created_at)
    WHERE status IN ('creating','container_ready','publishing','uncertain');
CREATE INDEX audit_event_idx ON audit_log (event_id, at DESC);

-- O banco também impede que outro escritor pule os portões do domínio.
CREATE FUNCTION guard_event_state() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'DETECTED' THEN
            RAISE EXCEPTION 'evento deve iniciar em DETECTED' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.state = OLD.state THEN
        RETURN NEW;
    END IF;
    IF NOT (
        (OLD.state = 'DETECTED' AND NEW.state = 'CLUSTERED') OR
        (OLD.state = 'CLUSTERED' AND NEW.state = 'VERIFIED') OR
        (OLD.state = 'VERIFIED' AND NEW.state = 'DRAFTED') OR
        (OLD.state = 'DRAFTED' AND NEW.state = 'CHECKED') OR
        (OLD.state = 'CHECKED' AND NEW.state = 'RENDERED') OR
        (OLD.state = 'RENDERED' AND NEW.state = 'QUEUED') OR
        (OLD.state = 'QUEUED' AND NEW.state = 'PUBLISHING') OR
        (OLD.state = 'PUBLISHING' AND NEW.state = 'PUBLISHED') OR
        (OLD.state = 'PUBLISHED' AND NEW.state = 'NEEDS_CORRECTION') OR
        (OLD.state IN ('DETECTED','CLUSTERED','VERIFIED','DRAFTED','CHECKED','RENDERED','QUEUED','PUBLISHING')
            AND NEW.state IN ('REJECTED','SUPERSEDED','FAILED'))
    ) THEN
        RAISE EXCEPTION 'transição de evento não permitida' USING ERRCODE = '23514';
    END IF;
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

CREATE TRIGGER events_state_guard BEFORE INSERT OR UPDATE ON events
FOR EACH ROW EXECUTE FUNCTION guard_event_state();
