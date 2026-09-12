FROM ubuntu:24.04

COPY root-logging-tests /usr/local/bin/root-logging-tests

RUN set -eu; \
    bucket="$(date -u +%Y-%m-%d)"; \
    for entrypoint in standard reloadable; do \
        for destination in file both; do \
            for format in pretty json; do \
                for owner in 0 1000 65532 new; do \
                    directory="/logs/${entrypoint}-${destination}-${format}-${owner}"; \
                    mkdir -p "${directory}"; \
                    chown 1000:1000 "${directory}"; \
                    chmod 0750 "${directory}"; \
                    if [ "${owner}" != new ]; then \
                        logfile="${directory}/root-logging-test.${bucket}.log"; \
                        printf 'historical log\n' >"${logfile}"; \
                        chown "${owner}:${owner}" "${logfile}"; \
                        chmod 0640 "${logfile}"; \
                    fi; \
                done; \
            done; \
        done; \
    done

USER 0:0
VOLUME ["/logs"]
ENTRYPOINT ["/usr/local/bin/root-logging-tests", "--ignored", "--exact", "root_appends_to_existing_logs_without_changing_ownership", "--nocapture"]
