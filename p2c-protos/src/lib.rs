//! Pre-confirmation (P2C / post-pack) gRPC protos.
//!
//! Uses the same protobuf packages as Jito (`auth`, `packet`, `shared`, `block_engine`)
//! so gRPC paths stay drop-in compatible with Jito-speaking remotes.

pub mod pre_conf {
    pub mod auth {
        tonic::include_proto!("auth");
    }

    pub mod block_engine {
        tonic::include_proto!("block_engine");
    }

    pub mod packet {
        tonic::include_proto!("packet");
    }

    pub mod shared {
        tonic::include_proto!("shared");
    }
}
