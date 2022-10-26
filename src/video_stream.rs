use std::{
    fs::File,
    io::{self, BufReader},
    path::PathBuf,
};

use clap::Args;

use crate::host::StreamGenerator;

#[derive(Args, Debug)]
pub struct VideoStreamer {
    video: PathBuf,
}

impl StreamGenerator for VideoStreamer {
    type Stream = BufReader<File>;
    type Err = io::Error;

    fn new_stream(&self) -> Result<Self::Stream, Self::Err> {
        let file = File::open(&self.video)?;
        let reader = BufReader::new(file);
        Ok(reader)
    }
}
