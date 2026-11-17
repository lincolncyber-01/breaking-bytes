CREATE TABLE rendered_stories (
    event_id bigint NOT NULL REFERENCES events(id),
    revision bigint NOT NULL,
    template text NOT NULL,
    artifact_path text NOT NULL,
    content_hash bytea NOT NULL CHECK(octet_length(content_hash)=32),
    byte_length bigint NOT NULL CHECK(byte_length BETWEEN 1 AND 8388608),
    width int NOT NULL CHECK(width=1080),
    height int NOT NULL CHECK(height=1920),
    layout jsonb NOT NULL,
    image_provenance jsonb NOT NULL,
    dry_run bool NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(event_id,revision,template)
);

-- Toda mudança depois do render invalida o candidato antes da publicação.
CREATE FUNCTION rendered_evidence_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.content_revision<>OLD.content_revision AND NEW.state IN ('RENDERED','QUEUED') THEN
        INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify','verify:revision:'||NEW.id||':'||NEW.content_revision,jsonb_build_object('event_id',NEW.id)) ON CONFLICT(dedupe_key) DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER rendered_evidence_revision AFTER UPDATE OF content_revision ON events FOR EACH ROW EXECUTE FUNCTION rendered_evidence_changed();

CREATE OR REPLACE FUNCTION source_identity_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target_id bigint; new_revision bigint;
BEGIN
    IF ROW(NEW.origin_group,NEW.enabled,NEW.url,NEW.kind,NEW.weight,NEW.layer) IS NOT DISTINCT FROM ROW(OLD.origin_group,OLD.enabled,OLD.url,OLD.kind,OLD.weight,OLD.layer) THEN RETURN NEW; END IF;
    FOR target_id IN SELECT DISTINCT ea.event_id FROM event_articles ea
        JOIN article_sightings s ON s.article_id=ea.article_id JOIN events e ON e.id=ea.event_id
        WHERE s.source_id=NEW.id AND e.state IN ('CLUSTERED','VERIFIED','DRAFTED','CHECKED','RENDERED','QUEUED') LOOP
        UPDATE events SET content_revision=content_revision+1 WHERE id=target_id RETURNING content_revision INTO new_revision;
        INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify','verify:revision:'||target_id||':'||new_revision,jsonb_build_object('event_id',target_id))
            ON CONFLICT(dedupe_key) DO NOTHING;
    END LOOP;
    RETURN NEW;
END;
$$;
