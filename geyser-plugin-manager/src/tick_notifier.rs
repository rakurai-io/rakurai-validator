/// Module responsible for notifying plugins about ticks
use {
    crate::geyser_plugin_manager::GeyserPluginManager,
    agave_geyser_plugin_interface::geyser_plugin_interface::{
        ReplicaTickInfoV2, ReplicaTickInfoVersions, TickSource,
    },
    log::*,
    solana_clock::Slot,
    solana_entry::poh::PohEntry,
    solana_measure::measure::Measure,
    solana_metrics::*,
    solana_pubkey::Pubkey,
    std::sync::{Arc, RwLock},
};

pub struct TickNotifierImpl {
    plugin_manager: Arc<RwLock<GeyserPluginManager>>,
}

impl TickNotifierImpl {
    pub fn new(plugin_manager: Arc<RwLock<GeyserPluginManager>>) -> Self {
        Self { 
            plugin_manager,
        }
    }

    /// Create a callback function that can be passed to PohRecorder
    /// Returns Arc to allow cloning without holding locks
    /// The source parameter is a u32: 0 = PohRecorder, 1 = BlockstoreProcessor
    pub fn create_callback(
        tick_notifier: Arc<Self>,
    ) -> Arc<dyn Fn(Slot, u64, &PohEntry, Option<&Pubkey>, u32) + Send + Sync> {
        Arc::new(move |slot: Slot, tick_index: u64, poh_entry: &PohEntry, leader: Option<&Pubkey>, source_u32: u32| {
            // Convert u32 to TickSource enum
            let source = match source_u32 {
                0 => TickSource::PohRecorder,
                1 => TickSource::BlockstoreProcessor,
                _ => {
                    warn!("Unknown tick source value: {}, defaulting to PohRecorder", source_u32);
                    TickSource::PohRecorder
                }
            };
            tick_notifier.notify_tick(slot, tick_index, poh_entry, leader, source);
        })
    }

    pub fn notify_tick(
        &self,
        slot: Slot,
        tick_index: u64,
        poh_entry: &PohEntry,
        leader: Option<&Pubkey>,
        source: TickSource,
    ) {
        let mut measure = Measure::start("geyser-plugin-notify_tick");

        let plugin_manager = self.plugin_manager.read().unwrap();
        if plugin_manager.plugins.is_empty() {
            return;
        }

        // Build tick info with leader information and source
        let tick_info_v2 = Self::build_replica_tick_info_v2(
            slot, 
            tick_index, 
            poh_entry, 
            leader,
            source,
        );

        for plugin in plugin_manager.plugins.iter() {
            let mut measure_plugin = Measure::start("geyser-plugin-update-tick");
            // Use V2 version which includes the source field
            match plugin.update_tick(ReplicaTickInfoVersions::V0_0_2(&tick_info_v2), slot) {
                Err(err) => {
                    error!(
                        "Failed to notify tick at slot {}, tick_index {}, source {:?}, error: ({}) to plugin {}",
                        slot,
                        tick_index,
                        source,
                        err,
                        plugin.name()
                    )
                }
                Ok(_) => {
                    trace!(
                        "Successfully notified tick at slot {}, tick_index {}, source {:?} to plugin {}",
                        slot,
                        tick_index,
                        source,
                        plugin.name()
                    );
                }
            }
            measure_plugin.stop();
            inc_new_counter_debug!(
                "geyser-plugin-update-tick-us",
                measure_plugin.as_us() as usize,
                10000,
                10000
            );
        }
        measure.stop();
        inc_new_counter_debug!(
            "geyser-plugin-notify_tick-us",
            measure.as_us() as usize,
            10000,
            10000
        );
    }

    fn build_replica_tick_info_v2<'a>(
        slot: Slot,
        tick_index: u64,
        poh_entry: &'a PohEntry,
        leader: Option<&'a Pubkey>,
        source: TickSource,
    ) -> ReplicaTickInfoV2<'a> {
        ReplicaTickInfoV2 {
            slot,
            tick_index,
            num_hashes: poh_entry.num_hashes,
            hash: poh_entry.hash.as_ref(),
            leader: leader.map(|p| p.as_ref()), // Convert &Pubkey to &[u8]
            source,
        }
    }
}

