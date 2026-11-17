ALTER TABLE sources ADD COLUMN origin_group text;
ALTER TABLE events ADD COLUMN content_revision bigint NOT NULL DEFAULT 0;
ALTER TABLE events ADD COLUMN editorial_version bigint NOT NULL DEFAULT 0;
ALTER TABLE events ADD COLUMN verified_revision bigint;
ALTER TABLE events ADD COLUMN editorial_attempts int NOT NULL DEFAULT 0 CHECK (editorial_attempts BETWEEN 0 AND 2);
ALTER TABLE events ADD COLUMN assessment_json jsonb;
ALTER TABLE events ADD COLUMN draft_json jsonb;
ALTER TABLE events ADD COLUMN factcheck_json jsonb;
ALTER TABLE events ADD COLUMN last_verification_at timestamptz;

CREATE FUNCTION evidence_revision_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target_id bigint; new_revision bigint;
BEGIN
    IF TG_TABLE_NAME='event_articles' THEN
        UPDATE events SET content_revision=content_revision+1 WHERE id=NEW.event_id RETURNING content_revision INTO new_revision;
        INSERT INTO jobs(kind,dedupe_key,payload)
            SELECT 'verify','verify:revision:'||id||':'||new_revision,jsonb_build_object('event_id',id)
            FROM events WHERE id=NEW.event_id AND state IN ('VERIFIED','DRAFTED','CHECKED')
            ON CONFLICT(dedupe_key) DO NOTHING;
    ELSE
        FOR target_id IN SELECT event_id FROM event_articles WHERE article_id=NEW.article_id LOOP
            UPDATE events SET content_revision=content_revision+1 WHERE id=target_id RETURNING content_revision INTO new_revision;
            INSERT INTO jobs(kind,dedupe_key,payload)
                SELECT 'verify','verify:revision:'||id||':'||new_revision,jsonb_build_object('event_id',id)
                FROM events WHERE id=target_id AND state IN ('CLUSTERED','VERIFIED','DRAFTED','CHECKED')
                ON CONFLICT(dedupe_key) DO NOTHING;
        END LOOP;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER event_article_revision AFTER INSERT ON event_articles FOR EACH ROW EXECUTE FUNCTION evidence_revision_changed();
CREATE TRIGGER article_sighting_revision AFTER INSERT ON article_sightings FOR EACH ROW EXECUTE FUNCTION evidence_revision_changed();

-- Metadados corrigidos invalidam decisões que já estavam em voo.
CREATE FUNCTION article_content_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target_id bigint; new_revision bigint;
BEGIN
    IF ROW(NEW.title,NEW.body_excerpt,NEW.published_at) IS NOT DISTINCT FROM ROW(OLD.title,OLD.body_excerpt,OLD.published_at) THEN RETURN NEW; END IF;
    FOR target_id IN SELECT event_id FROM event_articles WHERE article_id=NEW.id LOOP
        UPDATE events SET content_revision=content_revision+1 WHERE id=target_id RETURNING content_revision INTO new_revision;
        INSERT INTO jobs(kind,dedupe_key,payload)
            SELECT 'verify','verify:revision:'||id||':'||new_revision,jsonb_build_object('event_id',id)
            FROM events WHERE id=target_id AND state IN ('CLUSTERED','VERIFIED','DRAFTED','CHECKED')
            ON CONFLICT(dedupe_key) DO NOTHING;
    END LOOP;
    RETURN NEW;
END;
$$;
CREATE TRIGGER article_content_revision AFTER UPDATE OF title,body_excerpt,published_at ON articles FOR EACH ROW EXECUTE FUNCTION article_content_changed();

CREATE FUNCTION source_identity_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target_id bigint; new_revision bigint;
BEGIN
    IF ROW(NEW.origin_group,NEW.enabled,NEW.url,NEW.kind,NEW.weight,NEW.layer) IS NOT DISTINCT FROM ROW(OLD.origin_group,OLD.enabled,OLD.url,OLD.kind,OLD.weight,OLD.layer) THEN RETURN NEW; END IF;
    FOR target_id IN SELECT DISTINCT ea.event_id FROM event_articles ea
        JOIN article_sightings s ON s.article_id=ea.article_id JOIN events e ON e.id=ea.event_id
        WHERE s.source_id=NEW.id AND e.state IN ('CLUSTERED','VERIFIED','DRAFTED','CHECKED') LOOP
        UPDATE events SET content_revision=content_revision+1 WHERE id=target_id RETURNING content_revision INTO new_revision;
        INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify','verify:revision:'||target_id||':'||new_revision,jsonb_build_object('event_id',target_id))
            ON CONFLICT(dedupe_key) DO NOTHING;
    END LOOP;
    RETURN NEW;
END;
$$;
CREATE TRIGGER source_identity_revision AFTER UPDATE OF origin_group,enabled,url,kind,weight,layer ON sources FOR EACH ROW EXECUTE FUNCTION source_identity_changed();
