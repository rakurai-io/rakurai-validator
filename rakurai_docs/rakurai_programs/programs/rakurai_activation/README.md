# Rakurai Activation Program

A multisig-based Solana smart contract for enabling or disabling the Rakurai scheduler. It also governs the commission on block rewards for both the client (Rakurai), which is currently set to 0, and the validator.

**Note:** The remaining block rewards after commission deduction are distributed to stakers via the [`RewardDistributionProgram`](../reward_distribution/README.md).

➤ For more details, refer to the [IDL file](./idl/rakurai_activation.json).

---

## 1. Deployed program ID

- **Mainnet**: [rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi](https://solscan.io/account/rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi)
- **Testnet**: [pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt](https://solscan.io/account/pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt?cluster=testnet)

---

## 2. Purpose

Each validator must create a **Rakurai Activation Account (RAA)** — a **PDA jointly controlled by both the validator and Rakurai**.

This account governs:

- Whether the validator is **actively using the Rakurai scheduler** to schedule blocks.
- The **commission percentage** the validator wants to retain from total block rewards.
- Rakurai's commission from total block rewards (set during initialization and read from the global **Rakurai Activation Config Account**). Rakurai plans to introduce a small commission in the future.

---

## 3. Multisig control

This program implements a 2-party asynchronous multisig:

- **Enabling the Rakurai scheduler** → Requires **2/2 multisig approval**. One party (validator or Rakurai) proposes, and the other approves.
- **Disabling the scheduler** → Can be done **unilaterally (1/2 multisig)**. Either party can act independently to disable.

> Unlike traditional multisig, both parties do not sign the same transaction. Instead, actions are proposed and approved via separate transactions.

---

## 4. Rakurai Activation Account creation

- The validator initializes their **RakuraiActivationAccount** PDA using:
  - Their **identity pubkey**
  - A seed constant
- During creation, the validator specifies:
  - `validator_commission_bps` (0–10000) — the share the validator wants to retain from total block rewards
  - The client's (Rakurai) commission is fetched from a global config account (**Rakurai Activation Config Account**), a PDA under the same program. This value is currently 0 bps, though Rakurai plans to charge a small commission on block rewards in the future.

Once created, this account:

- Authorizes Rakurai reward logic on-chain.
- Enables the validator to use Rakurai's scheduler for enhanced performance and MEV rewards.

---

## 5. Commission updates

- The validator may update their [**commission percentage**](../../cli/activation.md#33-update-commission) at any time.
- The updated commission applies either:
  - From the **current epoch**, if no [`RewardCollectionAccount`](../reward_distribution/README.md#31-rewardcollectionaccount-account-initialization) has been initialized yet.
  - Or from the **next epoch**, if one already exists.

---

## 6. Activation flow

1. **Enabling Rakurai:**
   - The validator submits an [`update_rakurai_activation_approval`](../../cli/activation.md#32-scheduler-control) transaction.
   - In response, Rakurai submits a transaction to approve and activate the Rakurai scheduler.

2. **Disabling Rakurai:**
   - Either party (Rakurai or the validator) can unilaterally disable the Rakurai scheduler.

3. **Re-enabling:**
   - Requires both the validator and Rakurai to propose and approve via new transactions.

> Activation status is respected in reward distribution and scheduling logic across Rakurai-integrated programs.

---

## 7. CLI tool

See the [Rakurai Activation CLI](../../cli/activation.md) for operator commands to:

- Initialize a Rakurai Activation Account.
- Update commission settings.
- Enable or disable the Rakurai scheduler.

Install steps are in the [CLI overview](../../cli/README.md#2-installation).

---
