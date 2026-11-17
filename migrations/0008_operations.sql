-- Acentos são preservados. Grafias alternativas devem ser cadastradas explicitamente.
CREATE FUNCTION policy_text(input text) RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
 SELECT trim(regexp_replace(lower(normalize(COALESCE(input,''),NFC)), '[^[:alnum:]]+', ' ', 'g'));
$$;
ALTER TABLE blocklist ADD COLUMN active bool NOT NULL DEFAULT true;
ALTER TABLE blocklist ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();
CREATE INDEX blocklist_active ON blocklist(kind,policy_text(value)) WHERE active;
CREATE FUNCTION source_is_blocked(target int) RETURNS bool LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM sources s JOIN blocklist b ON b.active AND b.kind='source' WHERE s.id=target AND policy_text(s.name)=policy_text(b.value));
$$;
CREATE FUNCTION event_is_blocked(target bigint) RETURNS bool LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM events e CROSS JOIN blocklist b WHERE e.id=target AND b.active AND policy_text(b.value)<>'' AND (
 (b.kind='source' AND EXISTS(SELECT 1 FROM event_articles ea JOIN article_sightings a ON a.article_id=ea.article_id JOIN sources s ON s.id=a.source_id WHERE ea.event_id=e.id AND policy_text(s.name)=policy_text(b.value)))
 OR (b.kind IN('term','topic') AND (
 strpos(' '||policy_text(concat_ws(' ',e.title_pt,e.summary_pt,e.entities::text))||' ',' '||policy_text(b.value)||' ')>0
 OR EXISTS(SELECT 1 FROM event_articles ea JOIN articles a ON a.id=ea.article_id WHERE ea.event_id=e.id AND strpos(' '||policy_text(concat_ws(' ',a.title,a.body_excerpt))||' ',' '||policy_text(b.value)||' ')>0)
 OR EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(e.assessment_json->'sources','[]'::jsonb)) s WHERE strpos(' '||policy_text(concat_ws(' ',s->>'source_name',s->>'title',s->>'excerpt'))||' ',' '||policy_text(b.value)||' ')>0)
 ))));
$$;
ALTER TABLE publications ADD COLUMN container_created_at timestamptz;
ALTER TABLE publications ADD COLUMN simulated_at timestamptz;
UPDATE publications SET simulated_at=published_at,published_at=NULL WHERE status='dry_run';
CREATE TABLE publication_resolution_receipts (
 request_id uuid PRIMARY KEY,
 publication_id bigint NOT NULL REFERENCES publications(id),
 payload_hash bytea NOT NULL CHECK(octet_length(payload_hash)=32),
 media_id text NOT NULL CHECK(media_id ~ '^[0-9]{1,64}$'),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE OR REPLACE FUNCTION revision_evidence_fresh(target bigint,age_hours int,simulation bool) RETURNS bool LANGUAGE sql STABLE AS $$
    SELECT NOT event_is_blocked(target) AND EXISTS(
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
                 OR NOT COALESCE(s.enabled,false) OR source_is_blocked(s.id)
                 OR s.url IS DISTINCT FROM c->'before'->>'source_url'
                 OR s.name IS DISTINCT FROM c->'before'->>'source_name'
                 OR s.kind IS DISTINCT FROM c->'before'->>'kind'
                 OR s.layer::text IS DISTINCT FROM c->'before'->>'layer'
                 OR s.weight IS DISTINCT FROM (c->'before'->>'weight')::smallint
                 OR s.origin_group IS DISTINCT FROM c->'before'->>'origin_group'
          )
    );
$$;
