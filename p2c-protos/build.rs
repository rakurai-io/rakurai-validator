use tonic_prost_build::configure;

fn main() -> Result<(), std::io::Error> {
    const PROTOC_ENVAR: &str = "PROTOC";
    if std::env::var(PROTOC_ENVAR).is_err() {
        #[cfg(not(windows))]
        unsafe {
            std::env::set_var(PROTOC_ENVAR, protobuf_src::protoc());
        }
    }

    let proto_base_path = std::path::PathBuf::from("protos");
    // Separate packages (auth / packet / shared / block_engine) are required for Jito
    // drop-in gRPC paths: /auth.AuthService/..., /block_engine.BlockEngineRelayer/...
    let proto_files = [
        "shared.proto",
        "packet.proto",
        "auth.proto",
        "block_engine.proto",
    ];
    let mut protos = Vec::new();
    for proto_file in &proto_files {
        let proto = proto_base_path.join(proto_file);
        println!("cargo:rerun-if-changed={}", proto.display());
        protos.push(proto);
    }

    configure()
        .bytes(".packet.Packet.data")
        .build_client(true)
        .build_server(false)
        .compile_protos(&protos, &[proto_base_path])
}
