FROM ubuntu:focal AS build

RUN apt-get update -y && apt-get upgrade -y
RUN apt-get install -y git

# install rust
RUN apt-get install -y build-essential curl
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# install gRPC dependencies
RUN apt-get install -y protobuf-compiler

# install ROS
RUN apt-get install -y lsb-release
RUN sh -c 'echo "deb http://packages.ros.org/ros/ubuntu $(lsb_release -sc) main" > /etc/apt/sources.list.d/ros-latest.list'
RUN curl -s https://raw.githubusercontent.com/ros/rosdistro/master/ros.asc | apt-key add -
RUN apt-get update -y 
RUN DEBIAN_FRONTEND="noninteractive" apt-get install -y --no-install-recommends ros-noetic-desktop-full
RUN sh -c 'echo "source /opt/ros/noetic/setup.bash" >> ~/.bashrc'
RUN sh -c 'echo "source /opt/ros/noetic/setup.zsh" >> ~/.zshrc'

WORKDIR /cAR-host

ADD cAR-proto /cAR-host/cAR-proto
ADD src /cAR-host/src
ADD Cargo.lock /cAR-host/
ADD Cargo.toml /cAR-host/
ADD build.rs /cAR-host/