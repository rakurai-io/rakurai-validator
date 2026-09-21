# TX In/Out Feature

## Overview

The **TX In/Out** feature provides end-to-end visibility into transactions processed by the scheduler. It records all transaction signatures entering (`tx_in_signature`) and leaving (`tx_out_signature`) the scheduler, enabling users to independently confirm that:

- No transactions are censored.
- No transactions are selectively included.
- Transaction processing integrity is maintained.

This feature is primarily intended for validators using the `--tx-io-check` option and optionally integrates with an OMS (Order Management System) to sends txn signatures only of txns suffering non-recordable errors so that the side-car/OMS can generate status indication for customer txns that fail silently (i.e. without ending up in the ledger).

---

## How It Works

The feature consists of two components:

1. **Rust `HouseKeeper` Module**

   - Runs in a dedicated thread within the validator.
   - Monitors incoming (`tx_in_signature`) and outgoing (`tx_out_signature`) transactions.
   - Optionally writes the transactions to a log file (default: `/var/tmp/tx_io.log`).
   - Supports automatic log rotation once the file exceeds a maximum size (default 1 GB) and maintains backup logs.
   - Optional ZeroMQ PUB socket integration for sending transaction signatures to connected OMS clients.

2. **Python `tx_io_checker.py` Script**

   - Analyzes all transaction log files (`tx_io.log`, `tx_io.log.1`, `tx_io.log.2`, ...).
   - Counts total `tx_in_signature` and `tx_out_signature` occurrences.
   - Compares transaction signatures as multisets (taking duplicates into account).
   - Reports:
     - Number of transactions present in both `tx_in` and `tx_out`.
     - Transactions present only in `tx_in`.
     - Transactions present only in `tx_out`.
   - Detects excessive unmatched `tx_in` signatures at the end of the logs, highlighting potential censorship or dropped transactions.

---

## Prerequisites

- Rust validator built with `build_validator` feature enabled.
- Python 3.x for running the log analyzer.
- Optional: ZeroMQ for OMS integration.

---

# TX In/Out Feature - Usage Guide

### 1. Enable TX IO Checking

Start your validator with the `--tx-io-check` argument pointing to the desired log file path:

```bash
target/release/agave-validator --tx-io-check /path/to/tx_io.log
```

## 2. Run the Validator

1. Let the validator complete its turn(s).  
2. Ensure it enters **Forwarding mode** (next leader turn > 20 turns away).

---

## 3. Stop the Validator

Exit the validator using the CLI. This ensures the last transactions are flushed to the log file. Example exit command

```bash
agave-validator exit --max-delinquent-stake 10 --min-idle-time 60
```

---

## 4. Collect Log Files

Copy all `tx_io.log*` files from the validator machine:

```bash
cp /path/to/tx_io.log* /local/path/for/analysis/
```

> **Note:** Logs are overwritten on validator restart, so copy before restarting.

---

## 5. Analyze the Logs

Use the provided Python script to analyze the transaction logs:

```bash
python3 tx_io_checker.py /local/path/for/analysis/tx_io.log*
```

## Script Output

The script outputs:

- Total `tx_in_signature` and `tx_out_signature` counts.
- Whether the counts match.
- Multiset-aware comparison:
  - Transactions present in both.
  - Transactions only in `tx_in`.
  - Transactions only in `tx_out`.
- Detection of excessive unmatched `tx_in` signatures at the end.

### Example Output

```
Analyzing log files: /var/tmp/tx_io.log

Summary:
  tx_in_signature count : 97970
  tx_out_signature count: 97970
  Counts are equal

Comparison (multiset-aware):
  Present in both       : 97970
  Only in tx_in         : 0
  Only in tx_out        : 0

```

---

## 6. Optional: Custom Analysis Script

You may replace `tx_io_checker.py` with your own script, as long as it compares `tx_in` and `tx_out` logs in a similar multiset-aware manner.

---

## Log Rotation

- Default maximum log file size: 1 GB.
- Default number of backup logs retained: 7 (`tx_io.log.1`, `tx_io.log.2`, …).
- When the log exceeds the maximum size, it is rotated automatically, and older backups are shifted accordingly.

---

## ZeroMQ Integration (Optional)

- PUB socket path: `/tmp/txmeta.sock`.
- Maximum capacity of queue: 10,000 messages.

---

## Notes

- The `HouseKeeper` only runs if either `--tx-io-check` is enabled or OMS connector is active.
- The feature ensures that all logged transaction signatures are written in real-time to avoid data loss.
- Logs can be safely flushed and rotated without affecting validator operations.

---

## Summary

The **TX In/Out feature** provides a robust mechanism to audit transaction processing in Solana validators. By recording and analyzing transaction signatures, users gain full visibility into scheduler behavior, ensuring fairness and transparency.

## TX In/Out Localnet Test
A dedicated test is available for clients to run locally and also on demand in the GitHub Actions. This test spawns a localnet valaidator and matches input transaction hashes with output transaction hashes and confirms there is no injection or censoring of transactions inside the scheduler.

- Run the test
```bash
./tx_io_integrity_test.sh
```