FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:x86_64-musl as amd64
RUN sudo apt update && \
    apt install -y libssl-dev
WORKDIR /opt/speedupdate
COPY . .
RUN cargo build --release --verbose 
RUN mv target/x86_64-unknown-linux-musl/release/speedupdateserver target/release/speedupdateserver

FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:aarch64-musl as arm64
RUN sudo apt update && \
    apt install -y libssl-dev
WORKDIR /opt/speedupdate
COPY . .
RUN cargo build --release --verbose 
RUN mv target/aarch64-unknown-linux-musl/release/speedupdateserver target/release/speedupdateserver

FROM $TARGETARCH as build

FROM alpine:3.19 as final
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY --from=build /opt/speedupdate/target/release/speedupdateserver ./speedupdateserver

EXPOSE 3000
EXPOSE 3001
EXPOSE 50051
EXPOSE 2121
CMD ["./speedupdateserver"] 
