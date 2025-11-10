use serde::{Deserialize, Serialize};
use solana_perf::packet::PacketBatch;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PacketBundle {
    batch: PacketBatch,
    #[allow(unused)]
    bundle_id: String,
    block_engine_url: String,
}

impl PacketBundle {
    pub fn new(batch: PacketBatch, bundle_id: String, block_engine_url: String) -> Self {
        Self {
            batch,
            bundle_id,
            block_engine_url,
        }
    }

    pub fn batch(&self) -> &PacketBatch {
        &self.batch
    }

    pub fn block_engine_url(&self) -> &str {
        &self.block_engine_url
    }

    pub fn take(self) -> PacketBatch {
        self.batch
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedPacketBundle {
    batch: PacketBatch,
    block_engine_url: Option<String>,
}

impl VerifiedPacketBundle {
    pub fn new(batch: PacketBatch) -> Self {
        Self {
            batch,
            block_engine_url: None,
        }
    }

    pub fn new_with_block_engine_url(batch: PacketBatch, block_engine_url: String) -> Self {
        Self {
            batch,
            block_engine_url: (!block_engine_url.is_empty()).then_some(block_engine_url),
        }
    }

    pub fn take(self) -> PacketBatch {
        self.batch
    }

    pub fn batch(&self) -> &PacketBatch {
        &self.batch
    }

    pub fn block_engine_url(&self) -> &str {
        self.block_engine_url
            .as_deref()
            .unwrap_or("unknown")
    }
}
