//! Scheduler Test Setup
//! 
//! This module provides a test harness for the Rakurai scheduler that verifies the scheduler
//! does not perform any file I/O or network I/O syscalls. The test applies seccomp filters
//! to disable these syscalls before running the scheduler. If the scheduler attempts to perform
//! file or network operations, the process will be terminated, proving that the scheduler
//! operates without requiring these capabilities.
//! 
//! The test also verifies that transactions flow correctly through the scheduler by tracking
//! sent and received transaction signatures, ensuring the scheduler functions properly even
//! with these syscall restrictions in place.

use {
    seccomp_enforcer::apply_seccomp_filter_for_file_and_network_io,
    solana_core::{
        banking_stage::{
            decision_maker::DecisionMaker,
            RakuraiMode,
            scheduler_update_notifier,
            RakuraiConfig,
            SchedlingStrategy,
            reward_distributor::RewardDistributionConfig,
            reward_distributor::LatestBankPair,
            SchedulerObj,
            scheduler_messages::{ConsumeWork, FinishedConsumeWork},
            consume_worker::ConsumeWorkerMetrics,
            house_keeper::TxOutputStatus,
        },
        bundle_stage::bundle_account_locker::BundleAccountLocker,
        gui::GuiCoreMetrics,
        validator::ClientMode,
    },
    solana_perf::packet::bytes::Bytes,
    ahash::{HashSet as AHashSet, HashSetExt},
    agave_transaction_view::resolved_transaction_view::ResolvedTransactionView,
    crossbeam_channel::{bounded, unbounded, Receiver, Sender},
    solana_address::Address,
    solana_ledger::blockstore::Blockstore,
    solana_clock::DEFAULT_TICKS_PER_SLOT,
    solana_signature::Signature,
    solana_gossip::contact_info::ContactInfo,
    solana_transaction::Transaction,
    solana_keypair::Keypair,
    solana_signer::Signer,
    solana_runtime::bank::Bank,
    solana_runtime::installed_scheduler_pool::BankWithScheduler,
    solana_runtime::genesis_utils::create_genesis_config,
    solana_runtime_transaction::runtime_transaction::RuntimeTransaction,
    solana_transaction::versioned::VersionedTransaction,
    solana_perf::packet::to_packet_batches,
    solana_pubkey::Pubkey,
    solana_poh_config::PohConfig,
    solana_poh::poh_recorder::PohRecorder,
    solana_svm_transaction::svm_transaction::SVMStaticTransaction,
    agave_banking_stage_ingress_types::{BankingPacketBatch, BankingPacketReceiver},
    std::{
        collections::HashMap,
        thread::JoinHandle,
        sync::{
            atomic::AtomicU64,
            atomic::AtomicBool,
            atomic::Ordering,
            Arc, RwLock, Mutex,
        },
        time::Duration,
        thread::spawn,
    },
    solana_hash::Hash,
    solana_ledger::{
        leader_schedule_cache::LeaderScheduleCache,
        get_tmp_ledger_path_auto_delete,
    },
    solana_metrics as _,
    rand::Rng,
};

unsafe extern "C" {
    #[allow(improper_ctypes)]
    fn run_rakurai_scheduler(
        work_senders: Option<
            Vec<Sender<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>>,
        >,
        finished_work_receiver: Option<
            Receiver<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
        >,
        worker_metrics: Vec<Arc<ConsumeWorkerMetrics>>,
        high_priority_transaction_sender: Option<
            Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
        >,
        high_priority_transaction_receiver: Option<
            Receiver<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
        >,
        priority_threshold: Arc<AtomicU64>,
        decision_maker: DecisionMaker,
        contact_info: Arc<RwLock<ContactInfo>>,
        reward_distribution_config: RewardDistributionConfig,
        leader_schedule: Arc<LeaderScheduleCache>,
        non_vote_receiver: BankingPacketReceiver,
        rakurai_config: Arc<RwLock<RakuraiConfig>>,
        filter_keys: Arc<AHashSet<Pubkey>>,
        block_time_ms: u64,
        shared_bank_update: Arc<RwLock<LatestBankPair>>,
        output_tx_signature_sender: Option<Sender<TxOutputStatus>>,
        client_mode: Arc<Mutex<ClientMode>>,
        exit: Arc<AtomicBool>,
        nonce_packets: Arc<RwLock<HashMap<(Address, Hash), (Signature, u64)>>>,
        nonce_packet_receiver: Receiver<Signature>,
        poh_recorder: Arc<RwLock<PohRecorder>>,
        scheduling_strategy: Arc<SchedlingStrategy>,
        scheduler_update_sender: Option<
            Sender<
                scheduler_update_notifier::SchedulerUpdateWork<
                    RuntimeTransaction<ResolvedTransactionView<Bytes>>,
                >,
            >,
        >,
        bundle_account_locker: BundleAccountLocker,
        gui_core_metrics_sender: Option<Sender<GuiCoreMetrics>>,
    ) -> JoinHandle<()>;
}

fn main() {
    // Parse CLI arguments for number of transactions (default: 10)
    let num_transactions = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(10);
    
    println!("Running scheduler test to verify that the scheduler does not perform any file I/O or network I/O syscalls");
    // Track transaction signatures
    let sent_signatures = Arc::new(Mutex::new(Vec::new()));
    let received_signatures = Arc::new(Mutex::new(Vec::new()));

    let exit = Arc::new(AtomicBool::new(false));
    let ledger_path = get_tmp_ledger_path_auto_delete!();
    let blockstore = Blockstore::open(ledger_path.path())
    .expect("Expected to be able to open database ledger");

    let genesis = create_genesis_config(1_000_000_000);
    let mint_keypair = genesis.mint_keypair;
    let bank = Arc::new(Bank::new_for_tests(&genesis.genesis_config));
    let (poh_recorder, _entry_receiver) = PohRecorder::new(
        0,
        Hash::default(),
        bank.clone(),
        Some((4, 4)),
        DEFAULT_TICKS_PER_SLOT,
        Arc::new(blockstore),
        &Arc::new(LeaderScheduleCache::default()),
        &PohConfig::default(),
        Arc::new(AtomicBool::default()),
    );
    let poh_recorder = Arc::new(RwLock::new(poh_recorder));
    // DecisionMaker returns Consume iff shared leader state has a working bank.
    poh_recorder
        .write()
        .unwrap()
        .set_bank(BankWithScheduler::new_without_scheduler(bank.clone()));

    let contact_info: Arc<RwLock<ContactInfo>> = Arc::new(RwLock::new(ContactInfo::default()));

    let decision_maker = DecisionMaker::from(
        &poh_recorder,
    );

    let client_mode = Arc::new(Mutex::new(ClientMode::default()));
    let (non_vote_sender, non_vote_receiver) = unbounded();
    let block_time_ms = 350;
    let leader_schedule = Arc::new(LeaderScheduleCache::default());

    let (high_priority_transaction_sender, high_priority_transaction_receiver): (
        Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
        Receiver<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
    ) = crossbeam_channel::unbounded();
    let priority_threshold: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let num_workers = 3;
    let mut thread_hdls = Vec::with_capacity(num_workers + 1);

    const CHANNEL_CAPACITY: usize = 10_000;
    let (work_senders, work_receivers): (
        Vec<Sender<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>>,
        Vec<Receiver<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>>,
    ) = (0..num_workers).map(|_| bounded(CHANNEL_CAPACITY)).unzip();
    let (_finished_work_sender, finished_work_receiver): (
        Sender<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
        Receiver<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<Bytes>>>>,
    ) = bounded(num_workers.saturating_mul(CHANNEL_CAPACITY));

    let worker_metrics = Vec::with_capacity(num_workers);
    let shared_bank_update = Arc::new(RwLock::new(LatestBankPair::new(
        bank.clone(),
        bank.clone(),
    )));
    let last_blockhash = bank.last_blockhash();

    let rakurai_config = Arc::new(RwLock::new(RakuraiConfig {
        rs_mode: RakuraiMode::Mode1,
        rs_cfg_d1: 40,
        rs_cfg_ct1: 54,
        rs_cfg_nd1: 23,
        rs_cfg_ff1: 1.5,
        rs_cfg_ft1: 120,
        rs_cfg_tf1: 1.129,
        rs_cfg_ntft: 500,
        rs_cfg_nm: 0,
        rs_cfg_fp: 1,
    }));

    let nonce_packets = Arc::new(RwLock::new(HashMap::new()));
    let (_nonce_packet_sender, nonce_packet_receiver) = unbounded();

    let bundle_account_locker = BundleAccountLocker::default();
    let (scheduler_update_sender, _scheduler_update_receiver) =
    crossbeam_channel::unbounded();

    // Spawn the main test thread that sets up and runs the scheduler
    let handle = std::thread::spawn({
        let sent_signatures = sent_signatures.clone();
        let received_signatures = received_signatures.clone();
        move || {
            // Apply seccomp filter to restrict file and network I/O for security
            apply_seccomp_filter_for_file_and_network_io().expect("Failed to apply seccomp filter for file and network I/O");
            unsafe {
                thread_hdls.push(run_rakurai_scheduler(
                    Some(work_senders.clone()),
                    Some(finished_work_receiver.clone()),
                    worker_metrics,
                    Some(high_priority_transaction_sender),
                    Some(high_priority_transaction_receiver),
                    priority_threshold,
                    decision_maker,
                    contact_info,
                    RewardDistributionConfig::default(),
                    leader_schedule,
                    non_vote_receiver.clone(),
                    rakurai_config,
                    Arc::new(AHashSet::new()),
                    block_time_ms,
                    shared_bank_update.clone(),
                    None,
                    client_mode.clone(),
                    exit.clone(),
                    nonce_packets,
                    nonce_packet_receiver,
                    poh_recorder,
                    Arc::new(SchedlingStrategy::Strategy1),
                    Some(scheduler_update_sender),
                    bundle_account_locker.clone(),
                    None,
                ));
            }


            // Launch thread to send sample transactions to the scheduler
            let tx_sender_thread = {
                let non_vote_sender = non_vote_sender.clone();
                let exit = exit.clone();
                spawn(move || {
                    let mut count = 0;
                    while !exit.load(std::sync::atomic::Ordering::Relaxed) && count < num_transactions {
                        let packet_batch = BankingPacketBatch::new(
                            to_packet_batches(&[test_tx(&mint_keypair, last_blockhash)], 1)
                                .pop()
                                .expect("packet batch"),
                        );

                        // Send the batch to the scheduler
                        if let Err(_) = non_vote_sender.send(packet_batch.clone()) {
                            break; // Receiver dropped, exit thread
                        }

                        // Extract and track transaction signatures from the batch
                        for packet in packet_batch.iter() {
                            if let Some(Ok(versioned_transaction)) = packet
                                .data(..)
                                .map(bincode::deserialize::<VersionedTransaction>)
                            {
                                if let Some(signature) = versioned_transaction.signatures.first() {
                                    sent_signatures.lock().unwrap().push(signature.to_string());
                                }
                            }
                        }
                        count += 1;
                    }
                    // Wait 5 seconds before signaling shutdown
                    std::thread::sleep(Duration::from_secs(5));
                    exit.store(true, Ordering::Relaxed);
                })
            };

            // Launch threads to drain work receivers
            for (_index, work_receiver) in work_receivers.into_iter().enumerate() {
                let exit = exit.clone();
                let received_signatures = received_signatures.clone();
                thread_hdls.push(spawn(move || {
                    while !exit.load(Ordering::Relaxed) {
                        match work_receiver.recv() {
                            Ok(work) => {
                                // Extract and track signatures from received transactions
                                let txs = work.transactions;
                                for tx in txs {
                                        received_signatures.lock().unwrap().push(tx.signature().to_string()); 
                                }
                            }
                            Err(_) => {
                                // Channel closed, exit thread
                                break;
                            }
                        }
                    }
                }));
            }

            // Wait for transaction sender thread to complete
            tx_sender_thread.join().unwrap();
            // Close channels to signal workers to stop
            drop(work_senders);
            // Wait for all threads to complete
            for thread_hdl in thread_hdls {
                thread_hdl.join().unwrap();
            }
        }
    });

    // Join the main test thread and handle any panics
    match handle.join() {
        Ok(_) => {},
        Err(e) => {
            if let Some(s) = e.downcast_ref::<String>() {
                eprintln!("Scheduler test panicked: {}", s);
            } else if let Some(s) = e.downcast_ref::<&str>() {
                eprintln!("Scheduler test panicked: {}", s);
            } else {
                eprintln!("Scheduler test panicked with unknown error type {:?}", e);
            }
            std::process::exit(1);
        }
    }

    // Print the tracked signatures to verify transaction flow
    println!("Sent tx signatures: {:#?}", sent_signatures.lock().unwrap());
    println!("Received tx signatures: {:#?}", received_signatures.lock().unwrap());
    println!("Scheduler test completed successfully")
}

pub fn test_tx(fee_payer: &Keypair, last_blockhash: Hash) -> Transaction {
    let to = Keypair::new().pubkey();
    let lamports = rand::rng().random_range(1..=10000);
    solana_system_transaction::transfer(fee_payer, &to, lamports, last_blockhash)
}