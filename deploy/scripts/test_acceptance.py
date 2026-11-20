"""Contratos temporais e de integridade; relógio simulado não é aceite de produção."""
import copy
import datetime as dt
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from acceptance import ATTESTATIONS, DURATIONS, OperationError, approve, gate, observe, validate_report
from runtime import atomic_json, read_json

IMAGE = 'ghcr.io/lincolncyber-01/breaking-bytes@sha256:' + 'a' * 64


class Clock:
    def __init__(self, duration):
        self.value = 0
        self.start = dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=duration + 1)

    def monotonic(self):
        return self.value

    def wall(self):
        return (self.start + dt.timedelta(seconds=self.value)).isoformat()

    def sleep(self, seconds):
        self.value += seconds


class Client:
    deployment = {'image': IMAGE, 'config_sha256': 'b' * 64}

    def __init__(self, stage, bad=None):
        self.stage, self.index, self.bad = stage, 0, bad

    def sample(self):
        self.index += 1
        if self.bad == 'interrupt' and self.index == 3:
            raise KeyboardInterrupt()
        dry = self.stage in {'collection', 'dry_run'}
        sample = {'health': {'status': 'ready'}, 'status': {'dry_run': dry, 'paused': False, 'collection_enabled': True,
                  'editorial_enabled': True, 'imaging_enabled': True, 'publishing_enabled': True, 'publishing_ready': True, 'daily_cap': 10},
                  'stats': {'dry_run': dry, 'snapshot': {'articles': self.index, 'paused': False, 'dead_jobs': 0,
                  'sources': [{'silence_seconds': 0}], 'no_news': False, 'latency_high': False, 'publication_failures': False,
                  'metrics': [{'name': 'publications_total', 'label': 'dry_run' if dry else 'published', 'total': self.index}]}},
                  'peak_rss_bytes': 42_000_000, 'process_started_at': 'fixed-process'}
        if self.bad == 'mode':
            sample['status']['dry_run'] = not dry
        if self.bad == 'cap':
            sample['status']['daily_cap'] = 20
        if self.bad == 'restart' and self.index > 1:
            sample['process_started_at'] = 'changed-process'
        if self.bad == 'no_flow':
            sample['stats']['snapshot']['articles'] = 1
            sample['stats']['snapshot']['metrics'][0]['total'] = 1
        return sample


def report(stage, bad=None):
    clock = Clock(DURATIONS[stage])
    return observe(stage, '/path-that-does-not-exist/observation.json', Client(stage, bad),
                   clock.monotonic, clock.wall, clock.sleep, lambda *_: None)


class AcceptanceTests(unittest.TestCase):
    def test_complete_windows_require_flow_and_match_mode_cap_and_process(self):
        for stage in ['collection', 'dry_run', 'canary']:
            value = report(stage)
            self.assertTrue(value['passed'])
            validate_report(value)
            self.assertEqual(value['duration_seconds'], DURATIONS[stage])
        for bad in ['mode', 'cap', 'restart', 'no_flow']:
            value = report('canary', bad)
            self.assertFalse(value['passed'], bad)
            with self.assertRaises(OperationError):
                validate_report(value)

    def test_interruption_and_forged_duration_or_samples_cannot_approve(self):
        writes = []
        clock = Clock(DURATIONS['dry_run'])
        with self.assertRaises(KeyboardInterrupt):
            observe('dry_run', '/path-that-does-not-exist/observation.json', Client('dry_run', 'interrupt'),
                    clock.monotonic, clock.wall, clock.sleep, lambda _, value: writes.append(copy.deepcopy(value)))
        self.assertFalse(writes[-1]['completed'])
        value = report('dry_run')
        for field, change in [('duration_seconds', 30), ('samples', 1), ('availability', float('nan'))]:
            invalid = copy.deepcopy(value)
            invalid[field] = change
            with self.assertRaises(OperationError):
                validate_report(invalid)

    def test_human_review_hashes_sql_and_version_bind_the_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            values = {}
            for stage in ['collection', 'dry_run']:
                value = report(stage)
                values[stage] = value
                path = root / (stage + '.json')
                atomic_json(path, value)
                paths.append(path)
            review = {'reviewed_by': 'Operador de contrato', 'reviewed_at': dt.datetime.now(dt.timezone.utc).isoformat(),
                      'attestations': {key: True for key in ATTESTATIONS}, 'unsupported_claims': 0,
                      'semantic_duplicates': 0, 'false_acceptances': 0}
            sql = {'window_start': values['dry_run']['started_at'], 'window_end': dt.datetime.now(dt.timezone.utc).isoformat(),
                   'n1_articles': 10, 'n1_detect_p95_seconds': 50, 'n1_published': 0,
                   'n1_source_publish_p95_seconds': None, 'unresolved': 0, 'dead_jobs': 0,
                   'backup_verified': True, 'restore_verified': True}
            atomic_json(root / 'review.json', review)
            atomic_json(root / 'sql.json', sql)
            approve(paths, root / 'review.json', root / 'sql.json', root / 'approved.json')
            gate(root / 'approved.json', IMAGE, 'b' * 64)
            with self.assertRaises(OperationError):
                gate(root / 'approved.json', IMAGE, 'c' * 64)
            changed = read_json(paths[-1])
            changed['daily_cap'] = 80
            atomic_json(paths[-1], changed)
            with self.assertRaisesRegex(OperationError, 'evidencia_alterada'):
                gate(root / 'approved.json', IMAGE, 'b' * 64)
            self.assertEqual((root / 'approved.json').stat().st_mode & 0o777, 0o600)
