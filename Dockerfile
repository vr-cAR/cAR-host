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

# install gstreamer
RUN apt install -y \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav gstreamer1.0-doc gstreamer1.0-tools gstreamer1.0-x gstreamer1.0-alsa gstreamer1.0-gl gstreamer1.0-gtk3 gstreamer1.0-qt5 gstreamer1.0-pulseaudio

WORKDIR /cAR-host

ADD cAR-proto /cAR-host/cAR-proto
ADD src /cAR-host/src
ADD Cargo.lock /cAR-host/
ADD Cargo.toml /cAR-host/
ADD build.rs /cAR-host/