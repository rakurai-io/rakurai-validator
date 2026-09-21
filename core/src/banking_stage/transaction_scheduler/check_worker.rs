use {
    super::receive_and_buffer::{PrecheckResult, precheck_transaction},
    crossbeam_channel::{Receiver, Sender},
    solana_perf::packet::bytes::Bytes,
    solana_pubkey::Pubkey,
    solana_runtime::bank_forks::SharableBanks,
    std::{
        collections::HashSet,
        num::NonZeroUsize,
        sync::Arc,
        thread::{Builder, JoinHandle},
    },
};

pub(crate) fn spawn_check_workers(
    num_workers: NonZeroUsize,
    work_receiver: Receiver<(Bytes, i64, u32)>,
    result_sender: Sender<PrecheckResult>,
    sharable_banks: SharableBanks,
    filter_keys: Arc<HashSet<Pubkey>>,
) -> Vec<JoinHandle<()>> {
    (0..num_workers.get())
        .map(|index| {
            let work_receiver = work_receiver.clone();
            let result_sender = result_sender.clone();
            let sharable_banks = sharable_banks.clone();
            let filter_keys = filter_keys.clone();
            Builder::new()
                .name(format!("solBnkChk{index:02}"))
                .spawn(move || {
                    run_check_worker(work_receiver, result_sender, sharable_banks, filter_keys);
                })
                .expect("check worker thread must spawn")
        })
        .collect()
}

fn run_check_worker(
    work_receiver: Receiver<(Bytes, i64, u32)>,
    result_sender: Sender<PrecheckResult>,
    sharable_banks: SharableBanks,
    filter_keys: Arc<HashSet<Pubkey>>,
) {
    while let Ok((bytes, arrival_timestamp_nanos, source_ipv4)) = work_receiver.recv() {
        let banks = sharable_banks.load();
        let result =
            precheck_transaction(bytes, &banks.root_bank, &banks.working_bank, &filter_keys).map(
                |mut prechecked| {
                    // Preserve receive-time GUI metadata
                    prechecked
                        .state
                        .set_ingress_metadata(arrival_timestamp_nanos, source_ipv4);
                    prechecked
                },
            );

        // A result queue at capacity applies backpressure to check workers. Accepted
        // work is never dropped by a worker.
        if result_sender.send(result).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::banking_stage::tests::create_slow_genesis_config,
        crossbeam_channel::bounded,
        solana_ledger::genesis_utils::GenesisConfigInfo,
        solana_perf::packet::{BytesPacket, bytes::Bytes},
        solana_runtime::bank::Bank,
        solana_system_transaction::transfer,
    };

    fn test_banks() -> (SharableBanks, solana_keypair::Keypair) {
        let GenesisConfigInfo {
            genesis_config,
            mint_keypair,
            ..
        } = create_slow_genesis_config(u64::MAX);
        let (_bank, bank_forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);
        (bank_forks.read().unwrap().sharable_banks(), mint_keypair)
    }

    fn transaction_bytes(
        sharable_banks: &SharableBanks,
        mint_keypair: &solana_keypair::Keypair,
    ) -> Bytes {
        let transaction = transfer(
            mint_keypair,
            &Pubkey::new_unique(),
            1,
            sharable_banks.working().last_blockhash(),
        );
        BytesPacket::from_data(transaction)
            .unwrap()
            .buffer()
            .clone()
    }

    #[test]
    fn bounded_result_queue_does_not_drop_results() {
        let (sharable_banks, mint_keypair) = test_banks();
        let (work_sender, work_receiver) = bounded(8);
        let (result_sender, result_receiver) = bounded(1);
        let worker_handles = spawn_check_workers(
            NonZeroUsize::new(2).unwrap(),
            work_receiver,
            result_sender,
            sharable_banks.clone(),
            Arc::default(),
        );
        for _ in 0..8 {
            work_sender
                .send((
                    transaction_bytes(&sharable_banks, &mint_keypair),
                    0,
                    0,
                ))
                .unwrap();
        }

        for _ in 0..8 {
            let result = result_receiver.recv().unwrap();
            assert!(result.is_ok());
        }

        drop(work_sender);
        drop(result_receiver);
        worker_handles
            .into_iter()
            .for_each(|handle| assert!(handle.join().is_ok()));
    }

    #[test]
    fn ingress_metadata_survives_check_worker() {
        let (sharable_banks, mint_keypair) = test_banks();
        let (work_sender, work_receiver) = bounded(1);
        let (result_sender, result_receiver) = bounded(1);
        let worker_handles = spawn_check_workers(
            NonZeroUsize::new(1).unwrap(),
            work_receiver,
            result_sender,
            sharable_banks.clone(),
            Arc::default(),
        );

        let arrival = 42_i64;
        let source_ipv4 = u32::from(std::net::Ipv4Addr::new(10, 0, 0, 7));
        work_sender
            .send((
                transaction_bytes(&sharable_banks, &mint_keypair),
                arrival,
                source_ipv4,
            ))
            .unwrap();

        let prechecked = result_receiver.recv().unwrap().unwrap();
        assert_eq!(prechecked.state.arrival_timestamp_nanos(), arrival);
        assert_eq!(prechecked.state.source_ipv4(), source_ipv4);

        drop(work_sender);
        drop(result_receiver);
        worker_handles
            .into_iter()
            .for_each(|handle| assert!(handle.join().is_ok()));
    }
}
