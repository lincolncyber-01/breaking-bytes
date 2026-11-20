-- Consulta somente leitura. window_start deve vir de relatório UTC revisado.
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '10s';
SET LOCAL lock_timeout = '3s';
WITH detected AS (
    SELECT extract(epoch FROM a.detected_at-a.published_at)::float8 AS seconds
    FROM articles a JOIN sources s ON s.id=a.source_id
    WHERE s.layer<>'C' AND (s.layer='A' OR s.weight=5) AND a.detected_at>=:'window_start'::timestamptz
      AND a.published_at IS NOT NULL AND a.detected_at>=a.published_at
), published AS (
    SELECT extract(epoch FROM p.published_at-a.published_at)::float8 AS seconds
    FROM publications p JOIN events e ON e.id=p.event_id
    JOIN articles a ON a.id=e.primary_article_id
    WHERE NOT p.dry_run AND p.status='published' AND p.kind='new' AND e.level=1
      AND p.published_at>=:'window_start'::timestamptz AND a.published_at IS NOT NULL
      AND p.published_at>=a.published_at
)
SELECT json_build_object(
    'window_start', :'window_start'::timestamptz,
    'window_end', now(),
    'n1_articles', (SELECT count(*) FROM detected),
    'n1_detect_p95_seconds', (SELECT percentile_cont(0.95) WITHIN GROUP(ORDER BY seconds) FROM detected),
    'n1_published', (SELECT count(*) FROM published),
    'n1_source_publish_p95_seconds', (SELECT percentile_cont(0.95) WITHIN GROUP(ORDER BY seconds) FROM published),
    'unresolved', (SELECT count(*) FROM publications WHERE status IN('uncertain','published_unresolved','publishing')),
    'dead_jobs', (SELECT count(*) FROM jobs WHERE dead_at IS NOT NULL AND done_at IS NULL),
    'backup_verified', EXISTS(SELECT 1 FROM operational_backups WHERE verified_at>now()-interval '30 hours'),
    'restore_verified', EXISTS(SELECT 1 FROM operational_restore_checks WHERE checked_at>now()-interval '35 days')
);
COMMIT;
