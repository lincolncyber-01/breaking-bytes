"""Staging não herda banco ou token administrativo de produção."""
import unittest
from staging import isolated_environment
from runtime import OperationError


class StagingTests(unittest.TestCase):
    def environment(self):
        return {'STORIES_IMAGE': 'ghcr.io/lincolncyber-01/breaking-bytes@sha256:' + 'a' * 64,
                'IG_USER_ID': '123', 'STAGING_IG_USER_ID': '456',
                'DATABASE_URL': 'production', 'ADMIN_TOKEN': 'production',
                'STAGING_POSTGRES_PASSWORD': 'stage-only', 'STAGING_DATABASE_URL': 'stage-runtime',
                'STAGING_MIGRATION_DATABASE_URL': 'stage-owner', 'STAGING_ADMIN_TOKEN': 'stage-admin'}

    def test_dry_is_default_and_core_interpolation_uses_exclusive_credentials(self):
        source = self.environment()
        result = isolated_environment(source, False)
        self.assertEqual(result['STAGING_DRY_RUN'], 'true')
        self.assertEqual(result['DATABASE_URL'], 'stage-runtime')
        self.assertEqual(result['MIGRATION_DATABASE_URL'], 'stage-owner')
        self.assertEqual(result['POSTGRES_PASSWORD'], 'stage-only')
        self.assertEqual(result['ADMIN_TOKEN'], 'stage-admin')
        self.assertEqual(source['DATABASE_URL'], 'production')
        source.pop('STAGING_ADMIN_TOKEN')
        with self.assertRaises(OperationError):
            isolated_environment(source, False)

    def test_publish_requires_explicit_separate_numeric_account(self):
        source = self.environment()
        self.assertEqual(isolated_environment(source, True)['STAGING_DRY_RUN'], 'false')
        for value in ['123', '', 'https://example.invalid', '456\n']:
            source['STAGING_IG_USER_ID'] = value
            with self.assertRaises(OperationError):
                isolated_environment(source, True)
