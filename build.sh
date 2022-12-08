docker build --tag c-ar-host-build:latest --file build.Dockerfile . && \
    docker run --name c-ar-host-building c-ar-host-build:latest && \
    docker cp c-ar-host-building:/cAR-host/target/release/c-ar-host .