//! ## Overview
//!
//! The **TX In/Out** feature provides end-to-end visibility into transactions processed by the scheduler.
//! It records all transaction signatures entering (`tx_in_signature`) and leaving (`tx_out_signature`) the scheduler,
//! enabling users to independently confirm that:
//!
//! - No transactions are censored.
//! - No transactions are selectively included.
//! - Transaction processing integrity is maintained.
//!
//! This feature is primarily intended for validators using the `--tx-io-check` option and optionally integrates
//! with an OMS (Order Management System) to send txn signatures of transactions with non-recordable errors.
//! This allows a side-car/OMS to generate status indications for customer transactions that fail silently
//! (i.e., without appearing in the ledger).

#[cfg(feature = "build_validator")]
use crate::banking_stage::rakurai_enabled;

use {
    crate::banking_stage::{DecisionState, SharedDecision},
    chrono::Local,
    crossbeam_channel::{select, tick, Receiver},
    serde::{Deserialize, Serialize},
    solana_perf::packet::PacketRef,
    solana_transaction_error::TransactionError,
    std::{
        fs::{self, metadata, read_dir, remove_file, File},
        io::{self, BufWriter, Write},
        path::Path,
        string::String,
        sync::{
            atomic::{AtomicBool, Ordering::Relaxed},
            Arc,
        },
        thread::{self, Builder, JoinHandle},
        time::Duration,
    },
    zmq::Socket,
};

/// Enumerates all possible aggregated transaction errors tracked by the housekeeper.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[repr(C)]
pub enum AggregatedTxError {
    ConventionalErrorCode(TransactionError),
    DropOnReceive,
    DropOnCapacity,
    DropLeftOver,
    DropLeftOverDeserializer,
    Duplicate,
    SignatureVerificationFailed,
}

/// Represents a transaction output status with its signature and the result.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct TxOutputStatus {
    pub signature: String,
    pub status: Result<(), AggregatedTxError>,
}

/// Manages the background thread responsible for logging transaction I/O.
pub struct HouseKeeper {
    house_keeper_thread_hdl: JoinHandle<()>,
}

impl HouseKeeper {
    /// Launches the HouseKeeper background thread.
    /// It will only run if `tx_io_check` or `oms_connector` is enabled.
    pub fn new(
        input_tx_signature_receiver: Option<Receiver<String>>,
        output_tx_signature_receiver: Option<Receiver<TxOutputStatus>>,
        tx_io_check: Option<String>,
        oms_connector: bool,
        shared_decision: SharedDecision,
        exit: Arc<AtomicBool>,
    ) -> Self {
        let mut house_keeper_controller = HouseKeeperController {
            input_tx_signature_receiver,
            output_tx_signature_receiver,
            tx_io_check: tx_io_check.clone(),
            oms_connector,
            shared_decision,
            exit,
        };

        let house_keeper_thread_hdl = Builder::new()
            .name("solHouseKeeper".to_string())
            .spawn(move || {
                // Only run house keeper if either tx_io_check or oms_connector is enabled
                if tx_io_check.is_some() || oms_connector {
                    house_keeper_controller.run()
                }
            })
            .unwrap();

        Self {
            house_keeper_thread_hdl,
        }
    }

    /// Joins the HouseKeeper thread on termination to ensure clean shutdown.
    pub fn join(self) -> thread::Result<()> {
        self.house_keeper_thread_hdl.join()
    }
}

/// Internal controller struct handling the core HouseKeeper logic.
struct HouseKeeperController {
    input_tx_signature_receiver: Option<Receiver<String>>,
    output_tx_signature_receiver: Option<Receiver<TxOutputStatus>>,
    tx_io_check: Option<String>,
    #[allow(dead_code)]
    oms_connector: bool,
    shared_decision: SharedDecision,
    exit: Arc<AtomicBool>,
}

/// Default log file path and rotation parameters.
pub const DEFAULT_LOG_FILE: &str = "/var/tmp/tx_io.log";
const DEFAULT_MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024; // 1 GB
const DEFAULT_MAX_BACKUP_FILES: usize = 7;

/// Configuration for logging, including file path, max size, and backups.
#[derive(Debug)]
struct LogConfig {
    log_file: String,
    max_file_size_bytes: u64,
    max_backup_files: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_file: String::from(DEFAULT_LOG_FILE),
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE,
            max_backup_files: DEFAULT_MAX_BACKUP_FILES,
        }
    }
}

/// ZeroMQ PUB socket constants.
const ZMQ_FULL_PATH: &str = "PubSub_ipc:///tmp/txmeta.sock";
const ZMQ_SNDHWM: i32 = 10_000;

impl HouseKeeperController {
    /// Main loop for logging and managing TX I/O.
    fn run(&mut self) {
        let LogConfig {
            log_file,
            max_file_size_bytes,
            max_backup_files,
        } = self.get_tx_log_config();

        // Delete old log files on startup
        delete_previous_tx_logs_files(&log_file);

        // Create a new log file
        let mut writer = create_new_tx_file(&log_file);

        // Tick every second for periodic flushes and log rotation
        let ticker = tick(Duration::from_secs(1));

        #[allow(unused_variables)]
        let socket: Option<Socket> = create_zmq_socket();

        #[cfg(feature = "build_validator")]
        let mut rakurai_enabled_flag;
        // It must be Some if tx_io_check is true
        if let Some(output_tx_signature_receiver) = self.output_tx_signature_receiver.as_ref() {
            while !self.exit.load(Relaxed) {
                #[cfg(feature = "build_validator")]
                unsafe {
                    rakurai_enabled_flag = rakurai_enabled();
                }

                if self.tx_io_check.is_some() {
                    if let Some(input_tx_signature_receiver) =
                        self.input_tx_signature_receiver.as_ref()
                    {
                        select! {
                            // it must be Some if tx_io_check is true
                            recv(input_tx_signature_receiver) -> msg => {
                                if let Ok(_sig) = msg {
                                    #[cfg(feature = "build_validator")]
                                    {
                                        if rakurai_enabled_flag {
                                            write_input_msg(&mut writer, &_sig);
                                        }
                                    }
                                }
                            }
                            recv(output_tx_signature_receiver) -> msg => {
                                if let Ok(_tx_output_status) = msg {
                                    #[cfg(feature = "build_validator")]
                                    {
                                        if rakurai_enabled_flag {
                                            if self.oms_connector {
                                                if let Some(socket) = &socket {
                                                    let _ = send_tx_output(&socket, &_tx_output_status);
                                                }
                                            }
                                            write_output_msg(&mut writer, &_tx_output_status.signature, &_tx_output_status.status);
                                        }
                                    }
                                }
                            }
                            recv(ticker) -> _ => {
                                let _ = writer.flush();
                               // only rotate if tx_in and tx_out counts match
                               if let Ok(decision) = self.shared_decision.0.read() {
                                if let Ok(file_meta) = metadata(&log_file) {
                                    let size = file_meta.len();
                                    if *decision == DecisionState::Forward && size > max_file_size_bytes  {
                                        rotate_file(&log_file, &mut writer,max_backup_files)
                                            .unwrap_or_else(|e| warn!("failed to rotate tx_io.log: {}", e));

                                    }
                                }
                               }
                            }
                        }
                    }
                } else {
                    if let Ok(_tx_output_status) = output_tx_signature_receiver.recv() {
                        #[cfg(feature = "build_validator")]
                        {
                            if rakurai_enabled_flag {
                                if let Some(socket) = &socket {
                                    let _ = send_tx_output(&socket, &_tx_output_status);
                                }
                            }
                        }
                    }
                }
            }
            // Ensure flush on exit
            let _ = writer.flush();
        } else {
            warn!("HouseKeeperController: output_tx_signature_receiver is None, exiting");
        };
    }

    /// Determines preferred log file path and validates writability.
    fn get_tx_log_config(&self) -> LogConfig {
        let preferred_log_file = self.tx_io_check.clone().unwrap_or_default();

        let log_file = if !preferred_log_file.is_empty() {
            match Path::new(&preferred_log_file).parent() {
                Some(parent) => {
                    let writable = parent.exists() && parent.is_dir() && is_writable(parent);
                    if !writable {
                        warn!(
                            "Warning: No write permission in directory {}. Using default {}",
                            parent.display(),
                            DEFAULT_LOG_FILE
                        );
                        DEFAULT_LOG_FILE.to_string()
                    } else {
                        preferred_log_file
                    }
                }
                None => {
                    warn!(
                        "Warning: Could not determine parent directory of {}. Using default {}",
                        preferred_log_file, DEFAULT_LOG_FILE
                    );
                    DEFAULT_LOG_FILE.to_string()
                }
            }
        } else {
            DEFAULT_LOG_FILE.to_string()
        };

        LogConfig {
            log_file,
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE,
            max_backup_files: DEFAULT_MAX_BACKUP_FILES,
        }
    }
}

/// Check if a directory is writable by creating a temporary file.
fn is_writable(dir: &Path) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(".permission_check_tmp"))
        .map(|_| {
            let _ = std::fs::remove_file(dir.join(".permission_check_tmp"));
            true
        })
        .unwrap_or(false)
}

/// Writes an input transaction signature to the log file.
#[allow(dead_code)]
fn write_input_msg(writer: &mut BufWriter<File>, sig: &String) {
    if let Err(e) = writeln!(
        writer,
        "{} tx_in_signature: {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        sig
    ) {
        warn!("failed to write sig: {}", e);
    }
}

/// Writes an output transaction signature and status to the log file.
#[allow(dead_code)]
fn write_output_msg(
    writer: &mut BufWriter<File>,
    sig: &String,
    status: &Result<(), AggregatedTxError>,
) {
    if let Err(e) = writeln!(
        writer,
        "{} tx_out_signature: {} status: {:?}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        sig,
        status
    ) {
        warn!("failed to write sig: {}", e);
    }
}

/// Deletes all previous tx_io.log* files in the current directory.
fn delete_previous_tx_logs_files(log_file: &str) {
    let current_dir = Path::new(".");

    let entries = match read_dir(current_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to read current directory: {}", e);
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read directory entry: {}", e);
                continue;
            }
        };

        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => {
                warn!("Invalid filename encountered: {:?}", path);
                continue;
            }
        };

        // Check if file name matches
        if name == log_file || name.starts_with(&format!("{}.", log_file)) {
            if let Err(e) = remove_file(&path) {
                warn!("Failed to remove file {}: {}", name, e);
            } else {
                info!("Deleted file {}", name);
            }
        }
    }
}

/// Creates a new tx_io log file and wraps it in a buffered writer.
fn create_new_tx_file(log_file: &String) -> BufWriter<File> {
    let file = File::create(log_file)
        .unwrap_or_else(|_| File::create(DEFAULT_LOG_FILE.to_string()).unwrap());
    BufWriter::new(file)
}

/// Rotates the log file and manages backups.
pub fn rotate_file(path: &str, writer: &mut BufWriter<File>, max_backups: usize) -> io::Result<()> {
    if max_backups == 0 {
        return Ok(());
    }

    let base_path = Path::new(path);
    let file_name = match base_path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return Ok(()),
    };

    let oldest = base_path.with_file_name(format!("{}.{}", file_name, max_backups));
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }

    for i in (1..max_backups).rev() {
        let src = base_path.with_file_name(format!("{}.{}", file_name, i));
        let dst = base_path.with_file_name(format!("{}.{}", file_name, i + 1));
        if src.exists() {
            fs::rename(src, dst)?;
        }
    }

    if base_path.exists() {
        let first_backup = base_path.with_file_name(format!("{}.1", file_name));
        fs::rename(base_path, first_backup)?;
    }

    *writer = create_new_tx_file(&String::from(path));
    Ok(())
}

/// Creates a PUB socket for sending TX output over ZeroMQ.
pub fn create_zmq_socket() -> Option<Socket> {
    let context = zmq::Context::new();

    // Split full path into prefix and actual ZMQ path
    let (_prefix, zmq_path) = match ZMQ_FULL_PATH.split_once('_') {
        Some(parts) => parts,
        None => {
            warn!("Invalid ZMQ path format: {}", ZMQ_FULL_PATH);
            return None;
        }
    };

    if let Some(ipc_path) = zmq_path.strip_prefix("ipc://") {
        let path = Path::new(ipc_path);
        if path.exists() {
            if let Err(e) = std::fs::remove_file(path) {
                warn!("Failed to remove old IPC file {}: {:?}", ipc_path, e);
            }
        }
    }

    // Create the PUB socket
    let socket = match context.socket(zmq::PUB) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to create ZMQ PUB socket: {:?}", e);
            return None;
        }
    };

    // Bind the socket to the path
    if let Err(e) = socket.bind(zmq_path) {
        warn!("Failed to bind ZMQ PUB socket to {}: {:?}", zmq_path, e);
        return None;
    }

    // Set send high water mark
    if let Err(e) = socket.set_sndhwm(ZMQ_SNDHWM) {
        warn!("Failed to set ZMQ SNDHWM: {:?}", e);
        // Not critical, can continue
    }

    Some(socket)
}

/// Serializes and sends a message over the given ZMQ socket.
pub fn send_tx_output<T: Serialize>(socket: &Socket, msg: &T) -> Result<(), ()> {
    // Serialize the message
    let data = match bincode::serialize(msg) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to serialize message: {:?}", e);
            return Err(());
        }
    };

    // Send via ZMQ socket
    match socket.send(&data, zmq::DONTWAIT) {
        Ok(_) => Ok(()),
        Err(zmq::Error::EAGAIN) => Err(()),
        Err(e) => {
            warn!("Failed to send message via ZMQ socket: {:?}", e);
            Err(())
        }
    }
}

/// Serialize packet for logging (first 64 bytes as hex).
pub fn get_serialized_packet_for_logging(packet: &PacketRef<'_>) -> String {
    const FALLBACK_MSG: &str = "<invalid-packet>";

    let msg = match bincode::serialize(&packet.to_bytes_packet()) {
        Ok(bytes) => {
            // Take only the first 64 bytes, if available
            let prefix = bytes.iter().take(64);

            // Format as hex without spaces, like a compact signature
            prefix.map(|b| format!("{:02x}", b)).collect::<String>()
        }
        Err(_) => FALLBACK_MSG.to_string(),
    };
    msg
}
