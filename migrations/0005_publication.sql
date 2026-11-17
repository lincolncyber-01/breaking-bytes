-- Intenção local vinculada à conta, revisão e modo; nunca guardar URL assinada ou token.
ALTER TABLE publications DROP CONSTRAINT publications_status_check;
ALTER TABLE publications ADD CONSTRAINT publications_status_check CHECK(status IN(
    'creating','container_ready','publishing','uncertain','published','published_unresolved','dry_run','failed'
));
ALTER TABLE publications ADD COLUMN account_key text;
ALTER TABLE publications ADD COLUMN revision bigint;
ALTER TABLE publications ADD COLUMN artifact_hash bytea CHECK(octet_length(artifact_hash)=32);
ALTER TABLE publications ADD COLUMN reconciled_at timestamptz;
ALTER TABLE publications ADD COLUMN cleaned_at timestamptz;
ALTER TABLE publications ADD COLUMN remote_confirmed_at timestamptz;
ALTER TABLE publications ADD CONSTRAINT unresolved_confirmation CHECK(status<>'published_unresolved' OR
    (NOT dry_run AND container_id IS NOT NULL AND remote_confirmed_at IS NOT NULL AND published_at IS NOT NULL));
ALTER TABLE publications ADD CONSTRAINT mode_effects CHECK(NOT dry_run OR
    (container_id IS NULL AND media_id IS NULL AND request_started_at IS NULL));
ALTER TABLE events ADD COLUMN priority_signals jsonb;
CREATE INDEX publications_account_budget ON publications(account_key,created_at);
CREATE INDEX publications_cleanup ON publications(created_at) WHERE image_key IS NOT NULL AND cleaned_at IS NULL;
