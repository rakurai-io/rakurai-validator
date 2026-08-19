# Rakurai-Solana Docs

Welcome to the Rakurai documentation. These guides are intended for validator operators, searchers, and traders using the Rakurai ecosystem.

---

## 1. Background

### 1.1. What is Rakurai-Solana?

Rakurai-Solana is a high-performance Solana validator node designed to achieve **superior block rewards** and **higher Transactions Per Second (TPS)**. It incorporates heuristics-based **transaction scheduling** and other optimization techniques to efficiently process high-value transactions, boosting both performance and profitability for node operators.

### 1.2. High-Level Flow Architecture

The Rakurai node is composed of five main components:

1. **Rakurai Scheduler Library** — A scheduler optimized for selecting high-value transactions.
2. **Rakurai Agave Client** — A fork of the jito-solana client modified to run the Rakurai scheduler library.
3. **Rakurai Activation Program** — A smart contract that controls node participation and enables validators to run a Rakurai node.
4. **Reward Distribution Program** — Distributes block rewards to stakers via per-epoch **Reward Collection Accounts (RCA)** and post-epoch Merkle claims; tracks on-chain tip and MevShare revenue in per-validator, per-service **Tips Collection Accounts (TCA)** and **MevShare Collection Accounts (MCA)**.
5. **Rakurai Tip Manager Program** — Manages tips sent to Rakurai validators across eight tip PDAs; drains and splits tips into the validator's TCA.

### 1.3. Validator Incentives and Rewards Flow

With Rakurai's advanced transaction scheduler, validators can capture higher block rewards while improving both TPS and CU utilization. At present, Rakurai does not charge any fees for running its client. Validators may keep these rewards entirely or choose to share a portion with their stakers. Distribution is executed via a **configurable, trustless, merkle-root-based system**. In the future, Rakurai plans to charge a small commission on the block rewards earned by the validator.

### 1.4. How Rakurai Interacts with the Solana Ecosystem

Rakurai nodes function like standard Solana validators but include performance enhancements focused on transaction throughput and block reward optimization. They remain fully compatible with the Solana protocol while offering measurable improvements in validator economics. Rakurai actively maintains and updates the scheduler library to ensure compatibility with the latest Solana releases.

---

## 2. Documentation

| Section                                                               | Description                                                                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| [Validators](./rakurai_docs/validators/README.md)                     | Setup, operation, upgrades, binary attestation, and Geyser integration for Rakurai validators.               |
| [Transaction Inclusion](./rakurai_docs/transaction_inclusion/README.md) | Integrate with Rakurai transaction inclusion, bundle support, virtual priority, and post-pack confirmations. |
| [Programs](./rakurai_docs/rakurai_programs/README.md)                 | Documentation for Rakurai on-chain programs and protocol components.                                         |

## 3. Contacts

| Channel | Link |
| ------- | ---- |
| Website | [rakurai.io](https://rakurai.io) |
| Telegram | [t.me/rakurai_official](https://t.me/rakurai_official) |
| Discord | [discord.gg/XS7GmnmCJg](https://discord.gg/XS7GmnmCJg) |
| X | [@Rakurai_io](https://x.com/Rakurai_io) |
| LinkedIn | [Rakurai](https://www.linkedin.com/company/rakurai/) |
| GitHub | [rakurai-io/rakurai-validator](https://github.com/rakurai-io/rakurai-validator) |
