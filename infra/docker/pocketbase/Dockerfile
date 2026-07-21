ARG BUSYBOX_IMAGE
FROM ${BUSYBOX_IMAGE}

ARG POCKETBASE_VERSION
ARG POCKETBASE_URL
ARG POCKETBASE_SHA256
ARG AFK_UID
ARG AFK_GID
ADD ${POCKETBASE_URL} /tmp/pocketbase.zip
RUN echo "${POCKETBASE_SHA256}  /tmp/pocketbase.zip" | sha256sum -c - \
    && mkdir -p /usr/local/bin \
    && unzip /tmp/pocketbase.zip -d /usr/local/bin \
    && chmod 0555 /usr/local/bin/pocketbase \
    && rm /tmp/pocketbase.zip \
    && mkdir -p /afk/pb_data \
    && chown ${AFK_UID}:${AFK_GID} /afk/pb_data

LABEL org.opencontainers.image.source="https://github.com/pocketbase/pocketbase" \
      org.opencontainers.image.version="${POCKETBASE_VERSION}"
USER ${AFK_UID}:${AFK_GID}
EXPOSE 8090
ENTRYPOINT ["/usr/local/bin/pocketbase"]
