# Rakurai Activation Program

**This program is a validator's on/off switch for Rakurai, and where block reward commission is set.** Nothing else grants a node the right to run the Rakurai scheduler, and nothing else determines how a block reward splits between the validator, Rakurai, and stakers.

Control is split between the two parties: **enabling** the scheduler needs approval from both Rakurai and the validator, while **disabling** it can be done by either one alone. Commission is separate — **only the validator can change their own commission**, at any time. Rakurai's client commission is currently **0**.

**Note:** After commission, the remaining block rewards are shared with stakers through the [RCA](../reward_distribution/README.md#3-rca--block-rewards-for-stakers) in [Reward Distribution](../reward_distribution/README.md).

➤ For more details, refer to the [IDL file](./idl/rakurai_activation.json).

---

## 1. Deployed program ID

- **Mainnet**: [rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi](https://solscan.io/account/rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi)
- **Testnet**: [pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt](https://solscan.io/account/pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt?cluster=testnet)

---

## 2. Purpose

Each validator must create a **Rakurai Activation Account (RAA)** — a **PDA jointly controlled by both the validator and Rakurai**.

> [!WARNING]
> **One RAA per validator, per cluster**
>
> An RAA is derived from the **validator identity**, and each cluster uses a **different Rakurai Activation Program ID**. You therefore need a separate RAA for every validator *and* every cluster (testnet and mainnet-beta are separate accounts). Creating an RAA is mandatory before a Rakurai validator can run.

This account governs:

- Whether the validator is **actively using the Rakurai scheduler** to schedule blocks.
- The **commission percentage** the validator wants to retain from total block rewards.
- Rakurai's commission from total block rewards (set during initialization and read from the global **Rakurai Activation Config Account**). Rakurai plans to introduce a small commission in the future.

---

## 3. Multisig control

This program implements a 2-party asynchronous multisig:

- **Enabling the Rakurai scheduler** → Requires **2/2 multisig approval**. One party (validator or Rakurai) proposes, and the other approves.
- **Disabling the scheduler** → Can be done **unilaterally (1/2 multisig)**. Either party can act independently to disable.

> [!NOTE]
> **This multisig is asynchronous**
>
> Unlike a traditional multisig, both parties do **not** sign the same transaction. Each action is proposed in one transaction and approved in a separate one, so the validator and Rakurai never need to coordinate signing at the same moment.

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
  - From the **current epoch**, if no [RCA](../reward_distribution/README.md#3-rca--block-rewards-for-stakers) has been opened yet.
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

> [!NOTE]
> **Activation status is read by other programs**
>
> The enabled/disabled flag on the RAA is respected by reward distribution and scheduling logic across all Rakurai-integrated programs, not just by the scheduler itself. Disabling the scheduler therefore also stops Rakurai-side reward logic for that validator.

---

## 7. CLI tool

See the [Rakurai Activation CLI](../../cli/activation.md) for operator commands to:

- Initialize a Rakurai Activation Account.
- Update commission settings.
- Enable or disable the Rakurai scheduler.

Install steps are in the [CLI overview](../../cli/README.md#2-installation).

---
