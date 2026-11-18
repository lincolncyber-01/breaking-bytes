FROM amazon/aws-cli:2.39.6@sha256:eccdb3ec77e159d43ca6a750a70738c546883138be7f094498294a1477ec7e42 AS aws
FROM python:3.15.0-slim-trixie@sha256:f5b92f2a669a4c347044f29367f31983280b648250ca94dea66f98c53bc87e58 AS build
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends build-essential ca-certificates curl libssl-dev zlib1g-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
RUN curl --fail --silent --show-error --location https://ftp.postgresql.org/pub/source/v16.15/postgresql-16.15.tar.gz -o postgres.tar.gz && echo '4f200ca23dfb120ff9838f13ce06014aad1d3c432d16ee9f93ab2000c0eeef7b  postgres.tar.gz' | sha256sum --check && tar -xzf postgres.tar.gz
WORKDIR /build/postgresql-16.15
RUN ./configure --prefix=/usr/local/pgsql --with-openssl --without-gssapi --without-ldap --without-readline --without-icu && make -j2 && make -C src/interfaces/libpq install && make -C src/bin/pg_dump install && make -C src/bin/psql install
COPY --from=aws /usr/local/aws-cli /usr/local/aws-cli
RUN ln -s /usr/local/aws-cli/v2/current/bin/aws /usr/local/bin/aws && find /usr/local/lib/python3.15/lib-dynload \( -name '_sqlite3*' -o -name '_tkinter*' -o -name '_curses*' -o -name 'readline.*' -o -name '_uuid.*' \) -delete && rm -rf /usr/local/lib/python3.15/sqlite3 /usr/local/lib/python3.15/tkinter /usr/local/lib/python3.15/curses /usr/local/lib/python3.15/ensurepip /usr/local/lib/python3.15/site-packages/pip* /usr/local/bin/pip* && find /usr/local/aws-cli \( -type f -o -type l \) \( -name 'readline.cpython-*.so' -o -name '_uuid.cpython-*.so' -o -name 'libreadline.so.*' -o -name 'libtinfo.so.*' -o -name 'libuuid.so.*' \) -delete && mkdir -p /ops/licenses /var/lib/stories-backup && cp COPYRIGHT /ops/licenses/PostgreSQL.txt && chown 65532:65532 /var/lib/stories-backup && chmod 0700 /var/lib/stories-backup
COPY deploy/backup/collect_runtime.py /build/collect_runtime.py
RUN LD_LIBRARY_PATH=/usr/local/lib:/usr/local/pgsql/lib python3 /build/collect_runtime.py

FROM gcr.io/distroless/base-nossl-debian13:nonroot@sha256:89815d620ac01d8de13cdaf04d19ed874447ca2296d4985b14b818f6e5e47854
COPY --from=build /runtime/ /
COPY --from=build /usr/local/ /usr/local/
COPY --from=build /ops/licenses /ops/licenses
COPY --from=build --chown=65532:65532 /var/lib/stories-backup /var/lib/stories-backup
COPY --chmod=0444 deploy/backup/backup.py /ops/backup.py
ENV PATH=/usr/local/pgsql/bin:/usr/local/bin:/usr/bin:/bin LD_LIBRARY_PATH=/usr/local/lib:/usr/local/pgsql/lib PYTHONDONTWRITEBYTECODE=1 AWS_EC2_METADATA_DISABLED=true BACKUP_WORK_DIR=/var/lib/stories-backup
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/python3", "-I", "/ops/backup.py"]
# Verifica o runtime final; a inspeção na etapa de build não substitui esta execução.
RUN ["/usr/local/bin/python3", "-I", "-c", "import ssl, subprocess; from zoneinfo import ZoneInfo; print(ZoneInfo('America/Bahia')); print(ssl.OPENSSL_VERSION); [subprocess.run([name, '--version'], check=True, timeout=10) for name in ('pg_dump', 'pg_restore', 'psql', 'aws')]"]

# O contrato executa a CLI real, mas a rede do build permite somente loopback.
RUN --network=none --mount=type=bind,source=deploy/backup/check_runtime.py,target=/check_runtime.py,ro ["/usr/local/bin/python3", "-I", "/check_runtime.py", "--check-minimal"]
