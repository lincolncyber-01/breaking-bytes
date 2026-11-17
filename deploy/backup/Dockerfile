FROM amazon/aws-cli:2.39.6@sha256:eccdb3ec77e159d43ca6a750a70738c546883138be7f094498294a1477ec7e42 AS aws
FROM pgvector/pgvector:0.8.2-pg16@sha256:00ba258a66dac104fd5171074a0084462a64a1369d8513f3d0a634e2f24d15bc
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=aws /usr/local/aws-cli /usr/local/aws-cli
RUN ln -s /usr/local/aws-cli/v2/current/bin/aws /usr/local/bin/aws && aws --version && pg_dump --version && python3 --version && mkdir -p /ops /var/lib/stories-backup && chown 65532:65532 /var/lib/stories-backup && chmod 0700 /var/lib/stories-backup
COPY --chmod=0444 deploy/backup/backup.py /ops/backup.py
ENV PYTHONDONTWRITEBYTECODE=1 AWS_EC2_METADATA_DISABLED=true BACKUP_WORK_DIR=/var/lib/stories-backup
USER 65532:65532
ENTRYPOINT ["python3", "-I", "/ops/backup.py"]
