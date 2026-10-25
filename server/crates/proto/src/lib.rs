// The messages the server and frontend exchange, generated from protocol/gem.proto, with the
// canonical proto3 JSON mapping the frontend's protobuf-es decodes.

include!(concat!(env!("OUT_DIR"), "/gem.v1.rs"));
include!(concat!(env!("OUT_DIR"), "/gem.v1.serde.rs"));
