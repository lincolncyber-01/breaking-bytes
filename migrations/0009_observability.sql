-- Séries de cardinalidade controlada; nenhum texto jornalístico ou segredo vira label.
CREATE TABLE metric_totals (
    name text NOT NULL CHECK (name IN ('detect_latency_seconds','detect_to_publish_seconds','llm_latency_seconds','render_duration_seconds','articles_ingested_total','dedup_blocked_total','verification_rejected_total','factcheck_failed_total','ig_api_errors_total','publications_total')),
    label text NOT NULL CHECK (label ~ '^[a-zA-Z0-9_:]{1,32}$'),
    total double precision NOT NULL DEFAULT 0 CHECK (total >= 0 AND total < 'Infinity'::float8),
    samples bigint NOT NULL DEFAULT 0 CHECK (samples >= 0),
    buckets bigint[] NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (name,label)
);
CREATE TABLE metric_minutes (
    minute timestamptz PRIMARY KEY,
    samples bigint NOT NULL,
    slow bigint NOT NULL
);
CREATE FUNCTION record_metric(metric text,tag text,value double precision,observation bool DEFAULT false)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE limits double precision[] := ARRAY[0.005,0.01,0.05,0.1,0.15,0.5,1,5,30,60,300,600,3600,86400,'Infinity'::float8];
DECLARE counts bigint[];
BEGIN
    IF value < 0 OR value >= 'Infinity'::float8 THEN
        RAISE EXCEPTION 'amostra inválida' USING ERRCODE='23514';
    END IF;
    SELECT array_agg(CASE WHEN observation AND value<=bound THEN 1::bigint ELSE 0::bigint END ORDER BY ordinal)
      INTO counts FROM unnest(limits) WITH ORDINALITY AS bounds(bound,ordinal);
    INSERT INTO metric_totals(name,label,total,samples,buckets)
      VALUES(metric,tag,value,CASE WHEN observation THEN 1 ELSE 0 END,counts)
      ON CONFLICT(name,label) DO UPDATE SET total=metric_totals.total+EXCLUDED.total,
        samples=metric_totals.samples+EXCLUDED.samples,updated_at=now(),
        buckets=ARRAY(SELECT a+b FROM unnest(metric_totals.buckets,EXCLUDED.buckets) AS values_pair(a,b));
    IF metric='detect_to_publish_seconds' AND observation THEN
        INSERT INTO metric_minutes(minute,samples,slow)
          VALUES(date_trunc('minute',clock_timestamp()),1,CASE WHEN value>300 THEN 1 ELSE 0 END)
          ON CONFLICT(minute) DO UPDATE SET samples=metric_minutes.samples+1,slow=metric_minutes.slow+EXCLUDED.slow;
        DELETE FROM metric_minutes WHERE minute<now()-interval '24 hours';
    END IF;
END;
$$;
ALTER TABLE sources ADD COLUMN monitoring_since timestamptz NOT NULL DEFAULT now();
CREATE TABLE remote_quota (
    account_key text PRIMARY KEY,
    usage bigint NOT NULL CHECK(usage>=0),
    quota_limit bigint NOT NULL CHECK(quota_limit>0),
    checked_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE operational_alerts (
    code text NOT NULL CHECK(code ~ '^[a-z_]{1,40}$'),
    subject bigint NOT NULL DEFAULT 0 CHECK(subject>=0),
    active bool NOT NULL DEFAULT true,
    first_seen timestamptz NOT NULL DEFAULT now(),
    last_sent timestamptz,
    next_attempt timestamptz NOT NULL DEFAULT now(),
    failures int NOT NULL DEFAULT 0,
    lease_token uuid,
    lease_until timestamptz,
    PRIMARY KEY(code,subject),
    CHECK((lease_token IS NULL)=(lease_until IS NULL))
);
CREATE TABLE operational_backups (
    object_key text PRIMARY KEY,
    verified_at timestamptz NOT NULL DEFAULT now(),
    byte_length bigint NOT NULL CHECK(byte_length>0),
    sha256 text NOT NULL CHECK(sha256 ~ '^[a-f0-9]{64}$')
);
CREATE TABLE operational_restore_checks (
    id bigserial PRIMARY KEY,
    checked_at timestamptz NOT NULL DEFAULT now(),
    object_key text NOT NULL,
    target_system_id text NOT NULL CHECK(target_system_id ~ '^[0-9]{1,20}$')
);
ALTER TABLE pipeline_control ADD COLUMN restore_hold_until timestamptz;
ALTER TABLE pipeline_control ADD COLUMN operational_since timestamptz NOT NULL DEFAULT now();
CREATE FUNCTION guard_restore_hold() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT NEW.paused AND OLD.restore_hold_until>now() THEN
        RAISE EXCEPTION 'restauração exige quarentena' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER pipeline_restore_guard BEFORE UPDATE ON pipeline_control FOR EACH ROW EXECUTE FUNCTION guard_restore_hold();
CREATE FUNCTION observe_article() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE layer text;
BEGIN
    SELECT s.layer::text INTO layer FROM sources s WHERE s.id=NEW.source_id;
    PERFORM record_metric('articles_ingested_total',layer,1);
    IF NEW.published_at IS NOT NULL THEN
        PERFORM record_metric('detect_latency_seconds',layer,greatest(0,extract(epoch FROM NEW.detected_at-NEW.published_at)::float8),true);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER articles_metrics AFTER INSERT ON articles FOR EACH ROW EXECUTE FUNCTION observe_article();
CREATE FUNCTION observe_publication() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE layer text;
DECLARE event_level smallint;
DECLARE detected timestamptz;
BEGIN
    IF TG_OP='UPDATE' AND NEW.status=OLD.status THEN RETURN NEW; END IF;
    PERFORM record_metric('publications_total',NEW.status,1);
    IF NEW.status IN ('published','published_unresolved') AND (TG_OP='INSERT' OR OLD.status NOT IN ('published','published_unresolved')) THEN
        SELECT s.layer::text,COALESCE(e.level,0),COALESCE(r.created_at,a.detected_at,e.created_at)
          INTO layer,event_level,detected FROM events e
          LEFT JOIN articles a ON a.id=e.primary_article_id LEFT JOIN sources s ON s.id=a.source_id
          LEFT JOIN publication_reviews r ON r.id=e.review_id WHERE e.id=NEW.event_id;
        PERFORM record_metric('detect_to_publish_seconds',COALESCE(layer,'D')||':'||event_level::text,
          greatest(0,extract(epoch FROM COALESCE(NEW.published_at,now())-detected)::float8),true);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER publications_metrics AFTER INSERT OR UPDATE ON publications FOR EACH ROW EXECUTE FUNCTION observe_publication();
CREATE FUNCTION observe_editorial() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE reason text;
BEGIN
    IF NEW.state='REJECTED' AND OLD.state<>'REJECTED' THEN
        reason:=CASE WHEN NEW.reason IN ('policy_blocked','editorial_exhausted','unconfirmed_expired','evidence_expired','source_revision_obsolete','publication_evidence_expired','layout_exhausted') THEN NEW.reason ELSE 'other' END;
        PERFORM record_metric('verification_rejected_total',reason,1);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER events_metrics AFTER UPDATE ON events FOR EACH ROW EXECUTE FUNCTION observe_editorial();
CREATE FUNCTION observe_quality_block() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.step='rewrite_requested' AND NEW.detail->>'code' IN ('unsupported_number','unsupported_entity','unsupported_claim','unsupported_citation') THEN
        PERFORM record_metric('factcheck_failed_total','unsupported',1);
        INSERT INTO operational_alerts(code,subject) VALUES('unsupported_text',COALESCE(NEW.event_id,0)) ON CONFLICT DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER quality_metrics AFTER INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION observe_quality_block();

-- Privilégios específicos são aplicados somente quando o bootstrap foi executado.
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='stories_runtime') THEN
        REVOKE UPDATE,DELETE ON audit_log,publication_resolution_receipts FROM stories_runtime;
        REVOKE DELETE ON publications FROM stories_runtime;
        REVOKE INSERT,UPDATE,DELETE ON _sqlx_migrations FROM stories_runtime;
        GRANT DELETE ON metric_minutes,operational_alerts TO stories_runtime;
        REVOKE INSERT,UPDATE,DELETE ON pipeline_control FROM stories_runtime;
        GRANT UPDATE(paused,updated_at) ON pipeline_control TO stories_runtime;
    END IF;
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='stories_backup') THEN
        GRANT INSERT ON operational_backups,operational_restore_checks TO stories_backup;
    END IF;
END $$;
