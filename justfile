# vim: set ft=make :

build-dhat:
    cargo build --profile profiling --features dhat-heap
    @echo 'dhat binaries in target/profiling'
    @echo 'Example:'
    @echo 'WIRE_AGENT=/nix/store/...-agent-0.1.0 PROJECT/target/profiling/wire apply ...'
