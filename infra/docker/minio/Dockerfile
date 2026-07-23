ARG RUST_IMAGE
ARG BUSYBOX_IMAGE
FROM ${RUST_IMAGE} AS build

ARG MINIO_COMMIT
ARG MINIO_SHORT_COMMIT
ARG MINIO_VERSION
ARG MINIO_BUILD_VERSION
ARG MINIO_COPYRIGHT_YEAR
ARG MINIO_SOURCE_URL
ARG MINIO_SOURCE_SHA256
ARG GO_URL
ARG GO_SHA256
ADD ${GO_URL} /tmp/go.tar.gz
ADD ${MINIO_SOURCE_URL} /tmp/minio.tar.gz
RUN echo "${GO_SHA256}  /tmp/go.tar.gz" | sha256sum -c - \
    && echo "${MINIO_SOURCE_SHA256}  /tmp/minio.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/go.tar.gz -C /usr/local \
    && mkdir /src \
    && tar -xzf /tmp/minio.tar.gz --strip-components=1 -C /src
WORKDIR /src
ENV CGO_ENABLED=0
ENV PATH=/usr/local/go/bin:${PATH}
RUN mkdir -p /out \
    && go build -tags kqueue -trimpath \
    -ldflags="-s -w -X github.com/minio/minio/cmd.Version=${MINIO_BUILD_VERSION} -X github.com/minio/minio/cmd.CopyrightYear=${MINIO_COPYRIGHT_YEAR} -X github.com/minio/minio/cmd.ReleaseTag=${MINIO_VERSION} -X github.com/minio/minio/cmd.CommitID=${MINIO_COMMIT} -X github.com/minio/minio/cmd.ShortCommitID=${MINIO_SHORT_COMMIT} -X github.com/minio/minio/cmd.GOPATH=/go -X github.com/minio/minio/cmd.GOROOT=/usr/local/go" \
    -o /out/minio .

FROM ${BUSYBOX_IMAGE}
ARG MINIO_COMMIT
ARG MINIO_VERSION
ARG AFK_UID
ARG AFK_GID
LABEL org.opencontainers.image.source="https://github.com/minio/minio" \
      org.opencontainers.image.revision="${MINIO_COMMIT}" \
      org.opencontainers.image.version="${MINIO_VERSION}"
COPY --from=build /out/minio /usr/local/bin/minio
RUN chmod 0555 /usr/local/bin/minio \
    && mkdir -p /afk/minio-data \
    && chown ${AFK_UID}:${AFK_GID} /afk/minio-data
USER ${AFK_UID}:${AFK_GID}
EXPOSE 9000 9001
ENTRYPOINT ["/usr/local/bin/minio"]
