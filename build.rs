fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("cAR-proto/Control.proto")?;
    Ok(())
}
