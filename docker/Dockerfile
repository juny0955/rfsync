FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p cli -p daemon

FROM debian:bookworm-slim
COPY --from=build /src/target/release/rfsync /src/target/release/daemon /usr/local/bin/
# ./iden 과 ./example.db 가 이 디렉터리에 생긴다
WORKDIR /data
CMD ["sleep", "infinity"]
