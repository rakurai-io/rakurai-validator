use serde::{Deserialize, Serialize};
use solana_perf::packet::PacketBatch;
use std::time::SystemTime;

fn default_received_at() -> SystemTime {
    SystemTime::now()
}

/// Per-bundle options of the block engine connection a bundle arrived on, from the on-chain
/// `BlockEngineEntryV4`. The primary connection uses [`Self::PRIMARY`]; admin/CLI secondaries
/// use the default (`false`/`0`/`0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEngineBundleOptions {
    pub consider_primary: bool,
    pub sch: u8,
    pub consider: u8,
}

impl BlockEngineBundleOptions {
    /// Options stamped on bundles from the primary block engine connection.
    pub const PRIMARY: Self = Self {
        consider_primary: true,
        sch: 0,
        consider: 0,
    };

    /// Fallback for `sch = 0` (old configs, admin/CLI secondaries).
    pub const DEFAULT_SCH: usize = 2;
    /// Fallback for `consider = 0` (old configs, admin/CLI secondaries).
    pub const DEFAULT_CONSIDER: usize = 10;

    /// A secondary bundle is only processed while fewer than this many bundles have been
    /// processed in the current `consume_bundles` call.
    pub fn sch_limit(&self) -> usize {
        match self.sch {
            0 => Self::DEFAULT_SCH,
            sch => sch as usize,
        }
    }

    /// Examination window: a secondary bundle is only eligible within the first `consider`
    /// bundles examined in a `consume_bundles` call.
    pub fn consider_window(&self) -> usize {
        match self.consider {
            0 => Self::DEFAULT_CONSIDER,
            consider => consider as usize,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PacketBundle {
    batch: PacketBatch,
    bundle_id: String,
    /// Block engine connection identity (uuid). Empty for primary.
    block_engine_uuid: String,
    #[serde(skip, default = "default_received_at")]
    received_at: SystemTime,
    #[serde(default)]
    block_engine_options: BlockEngineBundleOptions,
}

impl PacketBundle {
    pub fn new(batch: PacketBatch, bundle_id: String, block_engine_uuid: String) -> Self {
        Self {
            batch,
            bundle_id,
            block_engine_uuid,
            received_at: SystemTime::now(),
            block_engine_options: BlockEngineBundleOptions::default(),
        }
    }

    pub fn with_block_engine_options(
        mut self,
        block_engine_options: BlockEngineBundleOptions,
    ) -> Self {
        self.block_engine_options = block_engine_options;
        self
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

    pub fn block_engine_options(&self) -> &BlockEngineBundleOptions {
        &self.block_engine_options
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
    block_engine_options: BlockEngineBundleOptions,
}

impl VerifiedPacketBundle {
    pub fn new(batch: PacketBatch) -> Self {
        Self {
            batch,
            block_engine_uuid: String::new(),
            bundle_id: String::new(),
            received_at: SystemTime::now(),
            // No block engine identity: treat like the primary connection.
            block_engine_options: BlockEngineBundleOptions::PRIMARY,
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
            block_engine_options: BlockEngineBundleOptions::default(),
        }
    }

    pub fn with_block_engine_options(
        mut self,
        block_engine_options: BlockEngineBundleOptions,
    ) -> Self {
        self.block_engine_options = block_engine_options;
        self
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

    pub fn block_engine_options(&self) -> &BlockEngineBundleOptions {
        &self.block_engine_options
    }
}
