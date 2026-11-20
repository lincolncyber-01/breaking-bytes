#!/usr/bin/env python3
"""Observação temporal real e recibo de revisão; não encurta janelas por argumento CLI."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time
from runtime import Admin, OperationError, Runner, atomic_json, finite, read_json, timestamp, utc_now

DURATIONS = {'collection': 86400, 'dry_run': 172800, 'canary': 259200, 'promoted': 259200}
ATTESTATIONS = {'source_terms', 'semantic_deduplication', 'editorial_fidelity', 'real_staging',
                'real_alert', 'real_backup_restore', 'kill_switch', 'peak_memory_load'}
IMAGE = re.compile(r'^ghcr\.io/lincolncyber-01/breaking-bytes@sha256:[a-f0-9]{64}$')


def digest(path):
    path = Path(path)
    read_json(path)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def counters(stats):
    result = {'articles': stats['snapshot']['articles'], 'dry_run': 0, 'published': 0, 'uncertain': 0}
    for metric in stats['snapshot']['metrics']:
        if metric['name'] == 'publications_total':
            label = metric['label']
            if label in {'dry_run', 'published'}:
                result[label] += metric['total']
            if label in {'uncertain', 'published_unresolved'}:
                result['uncertain'] += metric['total']
    if not all(finite(v) for v in result.values()):
        raise OperationError('contadores_invalidos')
    return result


class ObservedClient:
    def __init__(self, state_path):
        self.admin = Admin()
        self.runner = Runner()
        self.state_path = Path(state_path)
        self.deployment = read_json(self.state_path)['current']
        if not IMAGE.fullmatch(self.deployment['image']):
            raise OperationError('imagem_de_observacao_invalida')

    def sample(self):
        health = self.admin.request('/health/ready')
        status = self.admin.request('/admin/status')
        stats = self.admin.request('/admin/stats')
        current = read_json(self.state_path)['current']
        if current != self.deployment:
            raise OperationError('implantacao_mudou_durante_observacao')
        container = self.deployment['container_id']
        if not re.fullmatch(r'[a-f0-9]{64}', container):
            raise OperationError('container_invalido')
        raw = self.runner.run(['docker', 'inspect', '--format', '{{json .State}}', container], timeout=10, capture=True)
        state = json.loads(raw)
        pid = state.get('Pid')
        if state.get('Running') is not True or type(pid) is not int or pid <= 0:
            raise OperationError('processo_indisponivel')
        proc = Path(f'/proc/{pid}/status').read_text()[:16384]
        peak = next((int(line.split()[1]) * 1024 for line in proc.splitlines() if line.startswith('VmHWM:')), None)
        if not finite(peak):
            raise OperationError('pico_de_memoria_indisponivel')
        return {'health': health, 'status': status, 'stats': stats, 'peak_rss_bytes': peak,
                'process_started_at': state['StartedAt']}


def inspect_sample(stage, sample):
    status, stats = sample['status'], sample['stats']
    snapshot = stats['snapshot']
    dry = stage in {'collection', 'dry_run'}
    if sample['health'].get('status') != 'ready' or status.get('dry_run') is not dry or stats.get('dry_run') is not dry:
        raise OperationError('modo_ou_readiness_divergente')
    if status.get('paused') is not False or snapshot.get('paused') is not False or status.get('collection_enabled') is not True:
        raise OperationError('pipeline_pausado_ou_inativo')
    if stage != 'collection' and not all(status.get(key) is True for key in ['editorial_enabled', 'imaging_enabled', 'publishing_enabled']):
        raise OperationError('pipeline_incompleto')
    cap = status.get('daily_cap')
    if type(cap) is not int or not 1 <= cap <= 80 or stage == 'canary' and cap != 10 or stage == 'promoted' and cap not in {20, 40, 80}:
        raise OperationError('teto_divergente')
    if not dry and status.get('publishing_ready') is not True:
        raise OperationError('preflight_indisponivel')
    if snapshot.get('dead_jobs') != 0 or any(snapshot.get(key) is True for key in ['no_news', 'latency_high', 'publication_failures']):
        raise OperationError('sinal_operacional_critico')
    sources = snapshot.get('sources', [])
    if not sources or any(not finite(s.get('silence_seconds')) or s['silence_seconds'] > 600 for s in sources):
        raise OperationError('fonte_indisponivel')
    peak = sample.get('peak_rss_bytes')
    if not finite(peak, 1, 299_999_999):
        raise OperationError('orcamento_de_memoria_excedido')
    return counters(stats), cap, peak


def observe(stage, output, client, clock=time.monotonic, wall=utc_now, sleep=time.sleep, sink=atomic_json):
    if Path(output).exists():
        raise OperationError('relatorio_ja_existe')
    started = clock()
    report = {'schema': 1, 'stage': stage, 'required_seconds': DURATIONS[stage], 'started_at': wall(),
              'finished_at': None, 'duration_seconds': 0, 'completed': False, 'passed': False,
              'image': client.deployment['image'], 'config_sha256': client.deployment['config_sha256'],
              'samples': 0, 'healthy_samples': 0, 'critical_errors': {}, 'observations': [],
              'max_gap_seconds': 0, 'peak_rss_bytes': 0, 'daily_cap': None, 'flow_count': 0}
    initial = latest = None
    previous = started
    process_start = None
    while True:
        current = clock()
        elapsed = current - started
        gap = current - previous
        report['max_gap_seconds'] = max(report['max_gap_seconds'], gap)
        previous = current
        healthy, code = False, 'api_indisponivel'
        try:
            sample = client.sample()
            values, cap, peak = inspect_sample(stage, sample)
            if report['daily_cap'] is not None and report['daily_cap'] != cap:
                raise OperationError('teto_mudou')
            report['daily_cap'] = cap
            if process_start is not None and process_start != sample['process_started_at']:
                raise OperationError('processo_reiniciou')
            process_start = sample['process_started_at']
            report['peak_rss_bytes'] = max(report['peak_rss_bytes'], peak)
            if initial is None:
                initial = values
            if latest is not None and any(values[key] < latest[key] for key in values):
                raise OperationError('contadores_retrocederam')
            latest = values
            if values['uncertain'] > initial['uncertain']:
                raise OperationError('publicacao_incerta')
            healthy, code = True, 'ok'
        except OperationError as error:
            code = str(error)
            if code not in {'api_administrativa_indisponivel', 'comando_operacional_falhou', 'processo_indisponivel'}:
                report['critical_errors'][code] = report['critical_errors'].get(code, 0) + 1
        except (OSError, KeyError, ValueError, TypeError):
            code = 'amostra_invalida'
            report['critical_errors'][code] = report['critical_errors'].get(code, 0) + 1
        report['samples'] += 1
        report['healthy_samples'] += int(healthy)
        report['duration_seconds'] = elapsed
        report['observations'].append({'at_seconds': round(elapsed, 3), 'healthy': healthy, 'code': code})
        if len(report['observations']) > 5000:
            raise OperationError('amostras_acima_do_limite')
        if initial is not None and latest is not None:
            key = 'articles' if stage == 'collection' else 'dry_run' if stage == 'dry_run' else 'published'
            report['flow_count'] = latest[key] - initial[key]
        if elapsed >= DURATIONS[stage]:
            report['completed'] = True
            report['finished_at'] = wall()
            report['availability'] = report['healthy_samples'] / report['samples']
            report['passed'] = report['availability'] >= 0.995 and not report['critical_errors'] and report['max_gap_seconds'] <= 120 and report['flow_count'] > 0
            sink(output, report)
            return report
        sink(output, report)
        sleep(min(60, DURATIONS[stage] - elapsed))


def validate_report(value):
    stage = value.get('stage')
    if stage not in DURATIONS or value.get('schema') != 1 or value.get('completed') is not True or value.get('passed') is not True:
        raise OperationError('observacao_nao_aprovada')
    duration = value.get('duration_seconds')
    if not finite(duration, DURATIONS[stage], DURATIONS[stage] + 120) or value.get('required_seconds') != DURATIONS[stage]:
        raise OperationError('janela_temporal_incompleta')
    if not finite(value.get('availability'), 0.995, 1) or not finite(value.get('max_gap_seconds'), 0, 120) or value.get('critical_errors'):
        raise OperationError('observacao_com_falhas')
    if not finite(value.get('flow_count'), 1) or not finite(value.get('peak_rss_bytes'), 1, 299_999_999):
        raise OperationError('fluxo_ou_memoria_nao_demonstrado')
    wall = (timestamp(value['finished_at']) - timestamp(value['started_at'])).total_seconds()
    if abs(wall - duration) > 120 or not IMAGE.fullmatch(value.get('image', '')):
        raise OperationError('evidencia_temporal_divergente')
    if stage == 'canary' and value.get('daily_cap') != 10:
        raise OperationError('teto_divergente')
    observations = value.get('observations')
    if not isinstance(observations, list) or not 1 <= len(observations) <= 5000 or value.get('samples') != len(observations):
        raise OperationError('amostras_incompletas')
    previous, healthy = 0.0, 0
    for index, sample in enumerate(observations):
        point = sample.get('at_seconds')
        if not finite(point, previous, duration + 0.001) or point - previous > 120 or index == 0 and point > 1 or type(sample.get('healthy')) is not bool:
            raise OperationError('sequencia_temporal_invalida')
        previous = point
        healthy += int(sample['healthy'])
    if abs(previous - duration) > 0.01 or value.get('healthy_samples') != healthy or abs(value['availability'] - healthy / len(observations)) > 1e-9:
        raise OperationError('disponibilidade_divergente')
    return value


def validate_sql(sql, reports, maximum_age=30 * 3600):
    now = dt.datetime.now(dt.timezone.utc)
    end = timestamp(sql.get('window_end'))
    start = timestamp(sql.get('window_start'))
    if not -300 <= (now - end).total_seconds() <= maximum_age or start > end:
        raise OperationError('relatorio_sql_expirado')
    last = reports.get('promoted') or reports.get('canary') or reports['dry_run']
    if start > timestamp(last['started_at']) or end < timestamp(last['finished_at']):
        raise OperationError('janela_sql_divergente')
    if not finite(sql.get('n1_articles'), 1) or not finite(sql.get('n1_detect_p95_seconds'), 0, 119.999999):
        raise OperationError('slo_de_deteccao_nao_aprovado')
    if sql.get('unresolved') != 0 or sql.get('dead_jobs') != 0 or sql.get('backup_verified') is not True or sql.get('restore_verified') is not True:
        raise OperationError('operacao_nao_aprovada')
    if {'canary', 'promoted'} & reports.keys():
        if not finite(sql.get('n1_published'), 1) or not finite(sql.get('n1_source_publish_p95_seconds'), 0, 299.999999):
            raise OperationError('slo_de_publicacao_nao_aprovado')


def validate_review(review, maximum_age=86400):
    age = (dt.datetime.now(dt.timezone.utc) - timestamp(review.get('reviewed_at'))).total_seconds()
    if not isinstance(review.get('reviewed_by'), str) or not 1 <= len(review['reviewed_by'].strip()) <= 120 or not -300 <= age <= maximum_age:
        raise OperationError('revisao_humana_invalida')
    if set(review.get('attestations', {})) != ATTESTATIONS or not all(v is True for v in review['attestations'].values()):
        raise OperationError('atestados_incompletos')
    if any(type(review.get(key)) is not int or review[key] != 0 for key in ['unsupported_claims', 'semantic_duplicates', 'false_acceptances']):
        raise OperationError('qualidade_editorial_nao_aprovada')


def approve(reports, review_path, sql_path, output):
    if Path(output).exists():
        raise OperationError('recibo_ja_existe')
    review = read_json(review_path)
    validate_review(review)
    references = {}
    for path in reports:
        value = validate_report(read_json(path))
        if value['stage'] in references:
            raise OperationError('etapa_duplicada')
        references[value['stage']] = {'path': str(Path(path).resolve()), 'sha256': digest(path)}
    if not {'collection', 'dry_run'} <= references.keys():
        raise OperationError('observacoes_obrigatorias_ausentes')
    values = {stage: validate_report(read_json(reference['path'])) for stage, reference in references.items()}
    if len({(v['image'], v['config_sha256']) for v in values.values()}) != 1:
        raise OperationError('relatorios_de_versoes_diferentes')
    validate_sql(read_json(sql_path), values)
    receipt = {'schema': 1, 'reviewed_by': review['reviewed_by'], 'reviewed_at': review['reviewed_at'],
               'origin': 'atestado_humano_com_relatorios_locais', 'reports': references,
               'review': {'path': str(Path(review_path).resolve()), 'sha256': digest(review_path)},
               'sql': {'path': str(Path(sql_path).resolve()), 'sha256': digest(sql_path)}}
    atomic_json(output, receipt)
    return receipt


def gate(path, image, config_sha256, stage='dry_run', cap=None):
    receipt = read_json(path)
    age = (dt.datetime.now(dt.timezone.utc) - timestamp(receipt.get('reviewed_at'))).total_seconds()
    if receipt.get('schema') != 1 or receipt.get('origin') != 'atestado_humano_com_relatorios_locais' or not -300 <= age <= 7 * 86400:
        raise OperationError('recibo_expirado_ou_invalido')
    for reference in [*receipt.get('reports', {}).values(), receipt['review'], receipt['sql']]:
        if digest(reference['path']) != reference['sha256']:
            raise OperationError('evidencia_alterada')
    reference = receipt.get('reports', {}).get(stage)
    if reference is None:
        raise OperationError('etapa_de_aceite_ausente')
    report = validate_report(read_json(reference['path']))
    if report['image'] != image or report['config_sha256'] != config_sha256 or cap is not None and report['daily_cap'] != cap:
        raise OperationError('aceite_de_outra_versao')
    values = {name: validate_report(read_json(ref['path'])) for name, ref in receipt['reports'].items()}
    if not {'collection', 'dry_run'} <= values.keys() or any(v['image'] != image or v['config_sha256'] != config_sha256 for v in values.values()):
        raise OperationError('aceite_de_outra_versao')
    validate_review(read_json(receipt['review']['path']), 7 * 86400)
    validate_sql(read_json(receipt['sql']['path']), values)
    return receipt


def main():
    parser = argparse.ArgumentParser(description='Observação temporal e aprovação documentada do lançamento')
    commands = parser.add_subparsers(dest='command', required=True)
    observer = commands.add_parser('observe')
    observer.add_argument('stage', choices=sorted(DURATIONS))
    observer.add_argument('output', type=Path)
    observer.add_argument('--state', type=Path, default=Path('/var/lib/breaking-bytes/deployment.json'))
    approval = commands.add_parser('approve')
    approval.add_argument('--report', action='append', type=Path, required=True)
    approval.add_argument('--review', type=Path, required=True)
    approval.add_argument('--sql-report', type=Path, required=True)
    approval.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'observe':
            report = observe(args.stage, args.output, ObservedClient(args.state))
            print(json.dumps({'completed': report['completed'], 'passed': report['passed'], 'stage': args.stage}))
            return 0 if report['passed'] else 1
        approve(args.report, args.review, args.sql_report, args.output)
        print(json.dumps({'status': 'recibo_de_revisao_gravado'}))
        return 0
    except KeyboardInterrupt:
        print('Observação interrompida; o relatório parcial não aprova o lançamento.', file=sys.stderr)
        return 130
    except (OperationError, OSError, KeyError, ValueError, TypeError):
        print('Aceite recusado; confira duração, versão, integridade e evidências externas.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
