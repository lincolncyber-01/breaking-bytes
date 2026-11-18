//! Provisionamento explícito: migração, runtime e backup usam papéis distintos.
use crate::{Store, StoreError, database_error};
impl Store {
    /// Somente o comando administrativo chama esta rotina; credenciais nunca viram parâmetros de processo.
    pub async fn provision_roles(
        &self,
        owner_password: &str,
        runtime_password: &str,
        backup_password: &str,
    ) -> Result<(), StoreError> {
        if [owner_password, runtime_password, backup_password]
            .iter()
            .any(|p| !(32..=128).contains(&p.len()) || !p.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool().begin().await.map_err(database_error)?;
        let authorized:bool=sqlx::query_scalar("SELECT rolsuper AND current_user NOT IN('stories_owner','stories_runtime','stories_backup') FROM pg_roles WHERE rolname=current_user").fetch_one(&mut *tx).await.map_err(database_error)?;
        if !authorized {
            return Err(StoreError::InvalidOptions);
        }
        // Impede que o servidor registre o comando de senha, inclusive em falha SQL.
        sqlx::query("SET LOCAL log_statement='none'")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("SET LOCAL log_min_error_statement='panic'")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        for (role, password) in [
            ("stories_owner", owner_password),
            ("stories_runtime", runtime_password),
            ("stories_backup", backup_password),
        ] {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$1)")
                    .bind(role)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(database_error)?;
            if !exists {
                let statement:String=sqlx::query_scalar("SELECT format('CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION', $1::text)").bind(role).fetch_one(&mut *tx).await.map_err(database_error)?;
                sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            let statement:String=sqlx::query_scalar("SELECT format('ALTER ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L', $1::text,$2::text)").bind(role).bind(password).fetch_one(&mut *tx).await.map_err(database_error)?;
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        // AssertSqlSafe atesta somente instruções produzidas por format(%I/%L) no servidor.
        // Identificadores vêm de listas fixas; senhas já foram limitadas a hexadecimal.
        // Transfere apenas os objetos conhecidos das migrations deste projeto.
        let tables = vec![
            "_sqlx_migrations",
            "sources",
            "articles",
            "events",
            "event_articles",
            "publications",
            "jobs",
            "audit_log",
            "blocklist",
            "pipeline_control",
            "article_sightings",
            "rendered_stories",
            "token_health",
            "publication_watches",
            "publication_source_snapshots",
            "publication_reviews",
            "publication_resolution_receipts",
            "metric_totals",
            "metric_minutes",
            "remote_quota",
            "operational_alerts",
            "operational_backups",
            "operational_restore_checks",
        ];
        let statements:Vec<String>=sqlx::query_scalar("SELECT format('ALTER TABLE public.%I OWNER TO stories_owner',relname) FROM pg_class JOIN pg_namespace n ON n.oid=relnamespace WHERE n.nspname='public' AND relkind IN('r','p') AND relname=ANY($1)").bind(&tables).fetch_all(&mut *tx).await.map_err(database_error)?;
        for statement in statements {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        let functions = vec![
            "guard_event_state",
            "evidence_revision_changed",
            "article_content_changed",
            "source_identity_changed",
            "rendered_evidence_changed",
            "default_publication_kind",
            "revision_evidence_fresh",
            "publication_evidence_fresh",
            "policy_text",
            "source_is_blocked",
            "event_is_blocked",
            "record_metric",
            "guard_restore_hold",
            "observe_article",
            "observe_publication",
            "observe_editorial",
            "observe_quality_block",
        ];
        let statements:Vec<String>=sqlx::query_scalar("SELECT format('ALTER FUNCTION %s OWNER TO stories_owner',p.oid::regprocedure) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND proname=ANY($1) AND NOT EXISTS(SELECT 1 FROM pg_depend WHERE classid='pg_proc'::regclass AND objid=p.oid AND deptype='e')").bind(functions).fetch_all(&mut *tx).await.map_err(database_error)?;
        for statement in statements {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        // Todos os identificadores abaixo são constantes do projeto.
        for statement in [
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC",
            "GRANT USAGE,CREATE ON SCHEMA public TO stories_owner",
            "GRANT USAGE ON SCHEMA public TO stories_runtime,stories_backup",
            "ALTER DEFAULT PRIVILEGES FOR ROLE stories_owner IN SCHEMA public GRANT SELECT,INSERT,UPDATE ON TABLES TO stories_runtime",
            "ALTER DEFAULT PRIVILEGES FOR ROLE stories_owner IN SCHEMA public GRANT SELECT ON TABLES TO stories_backup",
            "ALTER DEFAULT PRIVILEGES FOR ROLE stories_owner IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO stories_runtime,stories_backup",
            "GRANT EXECUTE ON FUNCTION pg_catalog.pg_control_system() TO stories_backup",
        ] {
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        // Torna o bootstrap repetível também após migrações antigas executadas pelo administrador.
        for table in tables {
            let statement:String=sqlx::query_scalar("SELECT CASE WHEN to_regclass(format('public.%I',$1::text)) IS NULL THEN NULL ELSE format('GRANT SELECT,INSERT,UPDATE ON TABLE public.%I TO stories_runtime',$1::text) END").bind(table).fetch_optional(&mut *tx).await.map_err(database_error)?.flatten().unwrap_or_default();
            if !statement.is_empty() {
                sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                let revoke = statement
                    .replace("GRANT SELECT,INSERT,UPDATE", "REVOKE DELETE")
                    .replace(" TO stories_runtime", " FROM stories_runtime");
                sqlx::query(sqlx::AssertSqlSafe(revoke))
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                let statement = statement
                    .replace("SELECT,INSERT,UPDATE", "SELECT")
                    .replace("stories_runtime", "stories_backup");
                sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
        }
        sqlx::query("GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO stories_runtime,stories_backup").execute(&mut *tx).await.map_err(database_error)?;
        for (table, privileges) in [
            ("audit_log", "UPDATE,DELETE"),
            ("publication_resolution_receipts", "UPDATE,DELETE"),
            ("publications", "DELETE"),
            ("_sqlx_migrations", "INSERT,UPDATE,DELETE"),
            ("pipeline_control", "INSERT,UPDATE,DELETE"),
        ] {
            let statement:Option<String>=sqlx::query_scalar("SELECT CASE WHEN to_regclass(format('public.%I',$1::text)) IS NULL THEN NULL ELSE format('REVOKE %s ON TABLE public.%I FROM stories_runtime',$2::text,$1::text) END").bind(table).bind(privileges).fetch_one(&mut *tx).await.map_err(database_error)?;
            if let Some(statement) = statement {
                sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
        }
        let has_control: bool =
            sqlx::query_scalar("SELECT to_regclass('public.pipeline_control') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
        if has_control {
            sqlx::query("GRANT UPDATE(paused,updated_at) ON pipeline_control TO stories_runtime")
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        let has_backups: bool =
            sqlx::query_scalar("SELECT to_regclass('public.operational_backups') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
        if has_backups {
            sqlx::query("GRANT DELETE ON metric_minutes,operational_alerts TO stories_runtime")
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            sqlx::query(
                "GRANT INSERT ON operational_backups,operational_restore_checks TO stories_backup",
            )
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)
    }
    pub async fn runtime_role_safe(&self) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT NOT rolsuper AND NOT rolcreatedb AND NOT rolcreaterole AND NOT has_schema_privilege(current_user,'public','CREATE') AND NOT has_table_privilege(current_user,'public.audit_log','UPDATE') AND NOT has_table_privilege(current_user,'public.audit_log','DELETE') FROM pg_roles WHERE rolname=current_user")
            .fetch_one(self.pool()).await.map_err(database_error)
    }
}
