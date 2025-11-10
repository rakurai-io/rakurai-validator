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
            reward_distributor::RewardDistributionConfig,
            transaction_scheduler::transaction_state_container::SharedBytes,
            reward_distributor::LatestBankPair,
            SchedulerObj,
            scheduler_messages::{ConsumeWork, FinishedConsumeWork},
            DecisionState,
            LeaderMetaData,
            consume_worker::ConsumeWorkerMetrics,
            house_keeper::TxOutputStatus,
        },
        validator::ClientMode,

    },
    agave_transaction_view::resolved_transaction_view::ResolvedTransactionView,
    crossbeam_channel::{unbounded, Receiver, Sender},
    solana_gossip::contact_info::ContactInfo,
    solana_transaction::Transaction,
    solana_keypair::Keypair,
    solana_signer::Signer,
    solana_runtime::bank::Bank,
    solana_clock::Slot,
    solana_runtime_transaction::runtime_transaction::RuntimeTransaction,
    solana_transaction::versioned::VersionedTransaction,
    solana_genesis_config::GenesisConfig,
    solana_perf::packet::to_packet_batches,
    solana_pubkey::Pubkey,
    solana_svm_transaction::svm_transaction::SVMTransaction,
    agave_banking_stage_ingress_types::{BankingPacketBatch, BankingPacketReceiver},
    solana_runtime::bank::BANK_TEST_MODE,
    ahash::{HashSet, HashSetExt},
    std::{
        thread::JoinHandle,
        sync::{
            atomic::AtomicU64,
            atomic::AtomicBool,
            atomic::Ordering,
            Arc, RwLock, Mutex,
        },
        time::Duration,
        time::Instant,
        thread::spawn,
    },
    solana_hash::Hash,
    solana_ledger::{
        leader_schedule_cache::LeaderScheduleCache,
    },
    rand::Rng,
};

extern "C" {
    #[allow(improper_ctypes)]
    fn run_rakurai_scheduler(
        work_senders: Option<
            Vec<Sender<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>>,
        >,
        finished_work_receiver: Option<
            Receiver<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        >,
        worker_metrics: Vec<Arc<ConsumeWorkerMetrics>>,
        high_priority_transaction_sender: Option<
            Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        >,
        high_priority_transaction_receiver: Option<
            Receiver<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        >,
        priority_threshold: Arc<AtomicU64>,
        shared_decision: (Arc<RwLock<DecisionState>>, Arc<AtomicBool>),
        contact_info: Arc<RwLock<ContactInfo>>,
        reward_distribution_config: RewardDistributionConfig,
        leader_schedule: Arc<LeaderScheduleCache>,
        non_vote_receiver: BankingPacketReceiver,
        packet_delay: u64,
        blacklisted_accounts: HashSet<Pubkey>,
        block_time_ms: u64,
        shared_bank_update: Arc<RwLock<LatestBankPair>>,
        output_tx_signature_sender: Option<Sender<TxOutputStatus>>,
        client_mode: Arc<Mutex<ClientMode>>,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()>;
}

fn main() {
    // Parse CLI arguments for number of transactions (default: 10)
    let num_transactions = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(10);
    
    println!("Running scheduler test to verify that the scheduler does not perform any file I/O or network I/O syscalls");
    BANK_TEST_MODE.store(true, Ordering::Relaxed);
    // Track transaction signatures
    let sent_signatures = Arc::new(Mutex::new(Vec::new()));
    let received_signatures = Arc::new(Mutex::new(Vec::new()));

    let exit = Arc::new(AtomicBool::new(false));
    let bank = Arc::new(Bank::new_for_tests(&GenesisConfig::default()));
    let contact_info: Arc<RwLock<ContactInfo>> = Arc::new(RwLock::new(ContactInfo::default()));

    let shared_decision = (
        Arc::new(RwLock::new(DecisionState::Consume(LeaderMetaData {
            slot: Slot::default(),
            bank_creation_time: Instant::now(),
        }))),
        Arc::new(AtomicBool::new(false)),
    );
    let client_mode = Arc::new(Mutex::new(ClientMode::default()));
    let (non_vote_sender, non_vote_receiver) = unbounded();
    let block_time_ms = 350;
    let leader_schedule = Arc::new(LeaderScheduleCache::default());

    let (high_priority_transaction_sender, high_priority_transaction_receiver): (
        Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        Receiver<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
    ) = crossbeam_channel::unbounded();
    let priority_threshold: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let num_workers = 3;
    let mut thread_hdls = Vec::with_capacity(num_workers + 1);

    let (work_senders, work_receivers): (
        Vec<Sender<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>>,
        Vec<Receiver<ConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>>,
    ) = (0..num_workers).map(|_| unbounded()).unzip();
    let (_finished_work_sender, finished_work_receiver): (
        Sender<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        Receiver<FinishedConsumeWork<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
    ) = unbounded();

    let worker_metrics = Vec::with_capacity(num_workers);
    let shared_bank_update = Arc::new(RwLock::new(LatestBankPair::new(
        bank.clone(),
        bank.clone(),
    )));
    let last_blockhash = bank.last_blockhash();

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
                    shared_decision,
                    contact_info,
                    RewardDistributionConfig::default(),
                    leader_schedule,
                    non_vote_receiver.clone(),
                    0,
                    HashSet::new(),
                    block_time_ms,
                    shared_bank_update.clone(),
                    None,
                    client_mode.clone(),
                    exit.clone(),
                ));
            }

            // Launch thread to send sample transactions to the scheduler
            let tx_sender_thread = {
                let non_vote_sender = non_vote_sender.clone();
                let exit = exit.clone();
                spawn(move || {
                    let mut count = 0;
                    while !exit.load(std::sync::atomic::Ordering::Relaxed) && count < num_transactions {
                        let packet_batch = BankingPacketBatch::new(to_packet_batches(&vec![test_tx(last_blockhash); 1], 10));
                        
                        // Send the batch to the scheduler
                        if let Err(_) = non_vote_sender.send(packet_batch.clone()) {
                            break; // Receiver dropped, exit thread
                        }

                        // Extract and track transaction signatures from the batch
                        for batch in packet_batch.iter() {
                            for packet in batch {
                                if let Ok(versioned_transaction) =
                                    packet.deserialize_slice::<VersionedTransaction, _>(..)
                                {
                                    if let Some(signature) = versioned_transaction.signatures.first() {
                                        sent_signatures.lock().unwrap().push(signature.to_string());
                                    }
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
        Ok(_) => println!("Scheduler test completed successfully"),
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
}

// Generates a simple transfer transaction from a new keypair with a random lamports value between 1 and 10000
pub fn test_tx(last_blockhash: Hash) -> Transaction {
    let keypair1 = Keypair::new();
    let pubkey1 = keypair1.pubkey();
    let lamports = rand::thread_rng().gen_range(1..=10000);
    solana_system_transaction::transfer(&keypair1, &pubkey1, lamports, last_blockhash)
}