use solana_perf::packet::PacketBatch;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PacketBundle {
    pub batch: PacketBatch,
    pub bundle_id: String,
}
