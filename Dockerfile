# syntax=docker/dockerfile:1

FROM --platform=$BUILDPLATFORM tonistiigi/xx:1.10.0 AS xx

FROM --platform=$BUILDPLATFORM alpine:3.24 AS build
COPY --from=xx / /
RUN apk add --no-cache clang gcc lld musl-dev rustup
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
# rust-toolchain.toml is the only place that names the Rust version.
COPY rust-toolchain.toml .
RUN rustup-init -y --no-modify-path --profile minimal --default-toolchain none \
    && rustup toolchain install
ARG TARGETPLATFORM
RUN xx-apk add --no-cache gcc musl-dev
COPY . .
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/src/target,id=target-$TARGETPLATFORM \
    xx-cargo build --release --locked --package gw --target-dir target \
    && xx-verify --static target/$(xx-cargo --print-target-triple)/release/gw \
    && cp target/$(xx-cargo --print-target-triple)/release/gw /gw \
    && mkdir /data

FROM scratch AS binary
COPY --from=build /gw /gw

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /gw /usr/local/bin/gw
COPY --from=build --chown=nonroot:nonroot /data /data
ENV GW_HOME=/data
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["gw"]
CMD ["serve", "--listen", "0.0.0.0:8787"]
