use {
    crate::{
        banking_stage::committer::CommitTransactionDetails,
        gui::{
            metrics::{GuiTxnRecord, GuiTxnTpuSource},
            slot_txn::GuiTxnBatchItem,
        },
    },
    solana_svm_timings::ExecuteGuiTimestamps,
    solana_transaction_error::TransactionError,
};
/// GUI thread: build a display record from raw batch data.
pub fn gui_txn_record_from_batch_item(
    item: &GuiTxnBatchItem,
    gui_timestamps: &ExecuteGuiTimestamps,
) -> GuiTxnRecord {
    let mut record = GuiTxnRecord {
        signature: item.signature,
        compute_units_requested: item.compute_units_requested,
        bank_idx: item.bank_idx,
        is_simple_vote: item.is_simple_vote,
        timestamp_mb_start_nanos: item.timestamp_mb_start_nanos,
        timestamp_mb_end_nanos: item.timestamp_mb_end_nanos,
        timestamp_arrival_nanos: item.timestamp_arrival_nanos,
        source_ipv4: std::net::Ipv4Addr::from(item.source_ipv4),
        source_tpu: item.source_tpu,
        from_bundle: item.source_tpu == GuiTxnTpuSource::Bundle,
        timestamp_preload_end_nanos: gui_timestamps.timestamp_preload_end_nanos,
        timestamp_start_nanos: gui_timestamps.timestamp_start_nanos,
        timestamp_load_end_nanos: gui_timestamps.timestamp_load_end_nanos,
        timestamp_end_nanos: gui_timestamps.timestamp_end_nanos,
        ..GuiTxnRecord::default()
    };

    if let Some(detail) = &item.commit_detail {
        apply_commit_detail(&mut record, detail);
    }

    record
}

fn apply_commit_detail(record: &mut GuiTxnRecord, detail: &CommitTransactionDetails) {
    match detail {
        CommitTransactionDetails::Committed {
            compute_units,
            result,
            fee_details,
            tips,
            ..
        } => {
            record.compute_units_consumed = *compute_units as u32;
            record.transaction_fee = fee_details.transaction_fee();
            record.priority_fee = fee_details.prioritization_fee();
            record.landed = true;
            // Tips after ~6% block-builder commission; only for successful txs.
            record.tips = match result {
                Ok(()) => tips
                    .saturating_sub(tips.saturating_mul(6).saturating_div(100)),
                Err(_) => 0,
            };
            record.error_code = match result {
                Ok(()) => 0,
                Err(err) => transaction_error_to_gui_code(err),
            };
        }
        CommitTransactionDetails::NotCommitted(err) => {
            record.compute_units_consumed = 0;
            record.landed = false;
            record.error_code = transaction_error_to_gui_code(err);
        }
    }
}

/// Maps [`TransactionError`] to GUI `txn_error_code` values
fn transaction_error_to_gui_code(err: &TransactionError) -> u8 {
    match err {
        TransactionError::AccountInUse => 1,
        TransactionError::AccountLoadedTwice => 2,
        TransactionError::AccountNotFound => 3,
        TransactionError::ProgramAccountNotFound => 4,
        TransactionError::InsufficientFundsForFee => 5,
        TransactionError::InvalidAccountForFee => 6,
        TransactionError::AlreadyProcessed => 7,
        TransactionError::BlockhashNotFound => 8,
        TransactionError::InstructionError(..) => 9,
        TransactionError::CallChainTooDeep => 10,
        TransactionError::MissingSignatureForFee => 11,
        TransactionError::InvalidAccountIndex => 12,
        TransactionError::SignatureFailure => 13,
        TransactionError::InvalidProgramForExecution => 14,
        TransactionError::SanitizeFailure => 15,
        TransactionError::ClusterMaintenance => 16,
        TransactionError::AccountBorrowOutstanding => 17,
        TransactionError::WouldExceedMaxBlockCostLimit => 18,
        TransactionError::UnsupportedVersion => 19,
        TransactionError::InvalidWritableAccount => 20,
        TransactionError::WouldExceedMaxAccountCostLimit => 21,
        TransactionError::WouldExceedAccountDataBlockLimit => 22,
        TransactionError::TooManyAccountLocks => 23,
        TransactionError::AddressLookupTableNotFound => 24,
        TransactionError::InvalidAddressLookupTableOwner => 25,
        TransactionError::InvalidAddressLookupTableData => 26,
        TransactionError::InvalidAddressLookupTableIndex => 27,
        TransactionError::InvalidRentPayingAccount => 28,
        TransactionError::WouldExceedMaxVoteCostLimit => 29,
        TransactionError::WouldExceedAccountDataTotalLimit => 30,
        TransactionError::DuplicateInstruction(..) => 31,
        TransactionError::InsufficientFundsForRent { .. } => 32,
        TransactionError::MaxLoadedAccountsDataSizeExceeded => 33,
        TransactionError::InvalidLoadedAccountsDataSizeLimit => 34,
        TransactionError::ResanitizationNeeded => 35,
        TransactionError::ProgramExecutionTemporarilyRestricted { .. } => 36,
        TransactionError::UnbalancedTransaction => 37,
        TransactionError::ProgramCacheHitMaxLimit => 38,
        TransactionError::CommitCancelled => 39,
    }
}
