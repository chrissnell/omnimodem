fn main() {
    tonic_build::configure()
        .build_server(true)
        .build_client(true) // clients are generated for our own integration tests
        .compile_protos(&["../../proto/omnimodem.proto"], &["../../proto"])
        .expect("failed to compile omnimodem.proto");
}
