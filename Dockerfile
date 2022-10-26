FROM rust:latest AS build

RUN apt-get update -y && apt-get upgrade -y
RUN apt-get install -y protobuf-compiler

WORKDIR /cAR-host

ADD cAR-proto /cAR-host/cAR-proto
ADD src /cAR-host/src
ADD Cargo.lock /cAR-host/
ADD Cargo.toml /cAR-host/
ADD build.rs /cAR-host/

RUN cargo build --release
CMD ["./target/release/c-ar-host"]