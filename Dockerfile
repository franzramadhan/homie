# Built by .github/workflows/build.yml from the static musl release binaries
# in dist/<arch>/homie, so the image never compiles anything and needs no QEMU
# for arm64. Locally:
#   cargo build --release --target x86_64-unknown-linux-musl
#   mkdir -p dist/amd64 && cp target/x86_64-unknown-linux-musl/release/homie dist/amd64/
#   docker build --platform linux/amd64 -t homie .
FROM scratch
ARG TARGETARCH
COPY --chmod=0755 dist/${TARGETARCH}/homie /homie
USER 10001:10001
EXPOSE 7777
ENTRYPOINT ["/homie"]
