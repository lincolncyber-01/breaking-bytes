-- Somente metadados e hash criptográfico; nenhum token, recibo bruto ou URL.
CREATE TABLE token_health (
    account_key text NOT NULL CHECK(length(account_key) BETWEEN 1 AND 128),
    fingerprint bytea NOT NULL CHECK(octet_length(fingerprint)=32),
    observation jsonb NOT NULL CHECK(jsonb_typeof(observation)='object' AND octet_length(observation::text)<=16384),
    version bigint NOT NULL DEFAULT 1 CHECK(version>0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    refresh_lease_token uuid,
    refresh_lease_until timestamptz,
    last_refresh_attempt_at timestamptz,
    last_alert_at timestamptz,
    last_alert_status text CHECK(last_alert_status IN('unverified','valid','expiring','expired','invalid','stale','rotation_required')),
    PRIMARY KEY(account_key,fingerprint),
    CHECK((refresh_lease_token IS NULL)=(refresh_lease_until IS NULL))
);
