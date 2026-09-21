use serde::{Deserialize, Serialize};
use solana_perf::packet::PacketBatch;
use std::time::SystemTime;

fn default_received_at() -> SystemTime {
    SystemTime::now()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PacketBundle {
    batch: PacketBatch,
    bundle_id: String,
    /// Block engine connection identity (uuid). Empty for primary.
    block_engine_uuid: String,
    #[serde(skip, default = "default_received_at")]
    received_at: SystemTime,
}

impl PacketBundle {
    pub fn new(batch: PacketBatch, bundle_id: String, block_engine_uuid: String) -> Self {
        Self {
            batch,
            bundle_id,
            block_engine_uuid,
            received_at: SystemTime::now(),
        }
    }

    pub fn batch(&self) -> &PacketBatch {
        &self.batch
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn block_engine_uuid(&self) -> &str {
        &self.block_engine_uuid
    }

    pub fn received_at(&self) -> SystemTime {
        self.received_at
    }

    pub fn take(self) -> PacketBatch {
        self.batch
    }

    /// Consumes the bundle, returning batch, received_at, block_engine_uuid, and bundle_id.
    pub fn into_parts(self) -> (PacketBatch, SystemTime, String, String) {
        (
            self.batch,
            self.received_at,
            self.block_engine_uuid,
            self.bundle_id,
        )
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedPacketBundle {
    batch: PacketBatch,
    block_engine_uuid: String,
    pub bundle_id: String,
    received_at: SystemTime,
}

impl VerifiedPacketBundle {
    pub fn new(batch: PacketBatch) -> Self {
        Self {
            batch,
            block_engine_uuid: String::new(),
            bundle_id: String::new(),
            received_at: SystemTime::now(),
        }
    }

    pub fn new_with_bundle_id(
        batch: PacketBatch,
        block_engine_uuid: String,
        bundle_id: String,
    ) -> Self {
        Self::new_with_block_engine_uuid(batch, block_engine_uuid, bundle_id, SystemTime::now())
    }

    pub fn new_with_block_engine_uuid(
        batch: PacketBatch,
        block_engine_uuid: String,
        bundle_id: String,
        received_at: SystemTime,
    ) -> Self {
        Self {
            batch,
            block_engine_uuid,
            bundle_id,
            received_at,
        }
    }

    pub fn take(self) -> PacketBatch {
        self.batch
    }

    pub fn batch(&self) -> &PacketBatch {
        &self.batch
    }

    pub fn block_engine_uuid(&self) -> &str {
        &self.block_engine_uuid
    }

    pub fn received_at(&self) -> SystemTime {
        self.received_at
    }
}
