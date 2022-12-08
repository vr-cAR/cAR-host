# cAR-host

cAR-host sets up a server for streaming video and controls data through WebRTC. It does this through establishing a gRPC connection between two peers to coordinate the WebRTC handshake. The video pipelines are configurable through YAML. It is recommended however to build a gstreamer pipeline using `gst-inspect-1.0` and forward the RTP packets to the server to redirect to the client. 

Special thanks to the University of Texas Automata Group for lending their expertise and equipment for development throughout the entire pipeline. 
