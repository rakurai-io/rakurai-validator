# Rakurai Reward Distribution Program

**This is the accounting layer behind every Rakurai revenue stream.** It is how **four kinds of money** on a Rakurai validator get split and paid out: block rewards owed to stakers, plus the three [TIN](../../../transaction_inclusion/README.md) streams — tips, post-pack subscriptions, and backrun share.

Each stream gets its own account per validator, and for TIN partners per service, so what is owed and what has been paid is on-chain and checkable by both sides.

You do not need to know how the chain works to follow this. Each kind of money has its own **account** (a labeled wallet the program controls) and its own **flow**. They do not mix.

| | **RCA** | **TCA** | **PSA** | **MCA** |
|--|---------|---------|---------|---------|
| **Full name** | Reward Collection Account | Tips Collection Account | P2C Subscription Account | MevShare Collection Account |
| **What money is this?** | The validator’s **block rewards** | **Tips** traders and landing services pay to land transactions | Prepaid fee for **pre-conf / reselling** post-pack (**$50 / 1M Updates/txn**) | **Backrun / MevShare** profit share — alternate access path (no PSA) |
| **Who pays?** | The network (block rewards) | Traders / landing services | Resellers / pre-conf consumers | Backrun partners who share MevShare |
| **Why it exists** | Stakers should get their share of block rewards | Tips must be split: Rakurai cut, then the validator | Prepaid access so the leader-time stream stays on | Share backrun profit: Rakurai cut, then the validator (access without PSA) |
| **During the epoch** | Each time this validator leads, the last block’s reward is split; the **staker share** is parked in the RCA | Tips land in Rakurai tip accounts; Rakurai’s cut is taken; the **rest** is parked in the TCA | You **top up** the PSA so there is prepaid SOL sitting there | Nothing is collected automatically — profit sits with the user |
| **After the epoch** | A payout list is published; **stakers collect** | Remainder → validator (**high-priority block reward**, default) | Fee from prepaid: Rakurai’s cut, rest → validator (**high-priority block reward**, default) | User reports and sends shared profit; rest → validator (**high-priority block reward**, default) |
| **If it is not paid** | Unclaimed staker funds eventually return to the validator | Custom-tip partners who do not settle lose tip priority after a short grace | Stream is **stopped** until the balance is topped up | Users who do not share lose post-pack priority after a short grace |

Where to click: [P2C subscription](../../cli/p2c_subscription.md) (PSA) · [Partner settlement](../../cli/partner_reward_settlement.md) (TCA / MCA) · [Account layouts](#7-account-layouts) · [View on Solscan](#73-how-to-view-on-chain).

➤ On-chain interface file: [reward_distribution.json](./idl/reward_distribution.json).

> [!NOTE]
> **Post-pack access is PSA *or* MCA**
>
> **Pre-conf / reselling** → fund a **PSA** (leader-time stream). **Backrun / MevShare** → settle through an **MCA** and do **not** also pay a PSA; that path includes the TPU stream. Tips remain separate.

---

## 1. Deployed program ID

- **Mainnet**: [RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB](https://solscan.io/account/RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB)
- **Testnet**: [A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB](https://solscan.io/account/A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB?cluster=testnet)

---

## 2. Block reward conversion for TCA / MCA / PSA revenue

TCA, PSA, and MCA validator shares, after Rakurai’s commission, are first claimed and credited to the **validator identity account**.

With **block-reward conversion enabled by default**, that claimed amount is then converted into a **high-priority block-reward transaction** during the validator’s next leader turn. The effect is that revenue which arrived as a tip, a subscription fee, or a MevShare payment reaches stakers through the same block-reward path as ordinary block rewards.

The flag lives on each account individually (`block_reward_conversion_enabled` on the TCA, PSA, and MCA), so it can be on for one revenue stream and off for another on the same validator.

> [!WARNING]
> **The conversion transaction is not retried**
>
> The conversion must land **within that same leader turn**. If it does not, it is **dropped and not forwarded to the next leader** — the funds simply stay in the validator identity account instead of being converted.

> [!WARNING]
> **Do not count the same SOL twice**
>
> An indexer that watches **both** transfers into Rakurai tip accounts **and** validator block rewards will see the same lamports twice: once as the tip or claim, and again as the converted block reward. Count it once. Full explanation: [TIN — indexing note](../../../transaction_inclusion/README.md#6-do-not-double-count-tips-and-converted-block-rewards).

---

## 3. RCA — block rewards for stakers

### 3.1. Why it exists

When a validator produces blocks, Solana pays **block rewards**. Those rewards belong in part to **people who staked** with that validator, not only to the operator.

Solana has no native mechanism to pay block rewards out to stakers, so by default they stay in the validator identity account. The RCA is the holding account that fixes this: it collects the **stakers’ share** for one validator, for one epoch (~2 days), so it can be distributed after the epoch closes.

Operator-facing walkthrough: [Block Reward Distribution](../../../../block-rewards-distributor/block_reward_distribution.md).

### 3.2. Epoch flow

#### 3.2.1. RewardCollectionAccount initialization

On the first leader turn of each epoch, the `RewardCollectionAccount` is automatically initialized by the Rakurai Solana client. This initialization includes:

- Commission details, read from the validator-specific [`RakuraiActivationAccount`](../rakurai_activation/README.md#4-rakurai-activation-account-creation).
- The authority allowed to update the reward Merkle root. Only this authority can upload the Merkle root to the `RewardCollectionAccount`.

> [!NOTE]
> **Initialization is client-side**
>
> Account initialization logic lives in the Rakurai Solana client, not in a command you run. If the validator never leads during an epoch, no RCA is created for that epoch.

#### 3.2.2. Per-turn transfers

During every leader turn, the **previous turn’s block reward** is processed:

- **Client commission** → transferred to the client (Rakurai) account.
- **Validator commission** → remains in the validator’s identity account.
- **Staker share** → accumulated into the `RewardCollectionAccount`.

> [!NOTE]
> **Rewards lag one turn behind**
>
> Because the reward for the current turn is transferred during the *next* one, the **first turn of an epoch** settles the **last reward of the previous epoch**.

#### 3.2.3. Post-epoch staker distribution

At the final slot of each epoch:

1. A snapshot of Solana accounts is captured.
2. Each validator’s staker details and stake weights are extracted.
3. An off-chain Merkle tree is generated containing reward share data. At this stage specific stakers can be blacklisted and individual stake weights adjusted before the tree is finalized — see [custom distribution config](../../../../block-rewards-distributor/block_reward_distribution.md#6-customize-distribution-optional).
4. The Merkle root is uploaded to the `RewardCollectionAccount` by the `reward_merkle_root_authority`.
5. Stakers receive rewards via Merkle claims. When `reward_merkle_root_authority` is Rakurai, Rakurai runs the claim process on behalf of stakers.

### 3.3. Reward distribution — free and automated by Rakurai

- Set the Merkle root authority to [Rakurai](../../../validators/setup_and_build.md#5-add-additional-cli-args) for fully automated reward distribution.
- Keep it yourself if you want to run distribution manually.

When set to **Rakurai**, Rakurai will automatically:

1. **Create a snapshot**
2. **Calculate the Merkle root**
3. **Upload it on-chain**
4. **Run the claim process for stakers**

> [!TIP]
> **0% distribution fees charged by Rakurai**
>
> Delegating Merkle root authority to Rakurai costs nothing beyond standard Solana transaction fees.

### 3.4. Client commission on MEV rewards

The client charges commission on MEV rewards **only** if both of the following are true:

- The validator is actively running **Rakurai during that epoch**.
- The validator has set a non-zero **MEV commission** in their **Tip Distribution Account**.

> [!NOTE]
> **Zero MEV commission means zero Rakurai commission**
>
> If the validator’s MEV commission is **0%**, Rakurai does **not** charge any commission on MEV tips.

#### 3.4.1. Deduction flow

1. The validator’s share of MEV tips is credited to their **vote account** by the Tip Distribution Program in the following epoch.
2. A `ClaimStatus` account is created to track that the validator has received MEV rewards.
3. The Rakurai client monitors the `ClaimStatus` account; once it exists, commission becomes eligible for deduction.
4. Rakurai cannot deduct directly from the vote account, so the **same commission amount** is deducted from the validator’s **identity account** instead.
5. The deduction is performed by invoking the `transfer_client_commission_on_mev_commission` instruction in the Reward Distribution program.
6. The commission rate is defined in the **Reward Distribution Config account**.

---

## 4. TCA — tips for landing transactions

### 4.1. Why it exists

Traders and transaction-landing services pay a **tip** so the Rakurai scheduler will prioritize their transactions. Those tips must be split: **Rakurai gets a commission**, the **validator gets the rest**.

The TCA is where the **validator’s tip remainder** is collected for the epoch, then paid to the validator after the epoch ends (typically claimed in the next epoch).

There is one TCA per **service** per **validator**, so a validator that receives tips from several landing services has one TCA per service. Searcher-facing guide: [Tips](../../../transaction_inclusion/tips.md).

### 4.2. Working model — Rakurai tip accounts (usual case)

By default, services tip Rakurai’s [eight tip accounts](../rakurai_tip_manager/README.md), and `rakurai_tip_manager` drains them automatically.

1. A trader tips **any of the eight accounts**
2. Each time this validator is leader, those tip accounts are emptied
3. **Rakurai’s commission** is taken immediately
4. The **remainder** is moved into this validator’s TCA
5. After the epoch, that remainder is paid to the **validator’s identity**, then converted to a **high-priority block reward** (on by default)

Rakurai is not paid a second time at step 5 — the commission already happened at step 3.

### 4.3. Working model — custom tip account (partner)

Some landing services want tips in **their own** account. Rakurai cannot empty that account, so the money moves in the opposite direction: the partner pays in, rather than Rakurai draining.

1. You [register the account](../../../transaction_inclusion/tips.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts) and an agreed share with Rakurai (for example 30%)
2. During the epoch, the validator **writes down** what is owed (no SOL moves yet)
3. After the epoch, **you send** the owed SOL into the TCA
4. Then Rakurai’s commission is taken from what you sent, and the rest goes to the validator identity (same **block-reward conversion** as above)

> [!WARNING]
> **Unsettled custom tip accounts lose priority**
>
> If you do not settle within about **two epochs**, that custom tip account stops being used for prioritization from the next epoch onward. Settle with [`rakurai-revshare transfer --revenue-kind Tip`](../../cli/partner_reward_settlement.md#35-transfer--transfer-all).

Partner steps: [rakurai-revshare](../../cli/partner_reward_settlement.md) (`Tip`). On-chain layout: [TCA / MCA struct](#7-account-layouts).

---

## 5. PSA — prepaid fee to use post-pack

### 5.1. Why it exists

Partners who want **pre-conf / reselling** post-pack pay a **subscription** (PSA) for the stream. This is not a tip and not MevShare — it is the **price of that access path**, based on Updates/txn volume (**$50 / 1M txn updates**, dollar equivalent in SOL).

**Backrun / MevShare partners use an [MCA](#6-mca--sharing-post-pack-backrun-profit) instead and do not fund a PSA.**

From each epoch’s fee: **commission to Rakurai**, **remainder to the validator**.

There is one PSA per **service** per **validator**. Which servers receive the stream is configured separately in [Client Config](../rakurai_client_config/README.md). The PSA only holds the **prepaid SOL**.

Full product guide: [Post-pack confirmations](../../../transaction_inclusion/post_pack/README.md).

### 5.2. Working model

1. A **PSA** exists for your service + validator (created by Rakurai / ops; defaults from on-chain **`P2CConfigAccount`**)
2. **You top up** SOL into that account (`fund` / `fund-all`, or any wallet transfer)
3. Epoch ends. Update volume is priced and the fee due is written
4. The fee is taken from prepaid: Rakurai’s cut, rest to the validator identity (**block-reward conversion** on by default)
5. If the balance is too low, top up and try again — or the shortfall is booked as **deficit**
6. After a short grace, status becomes **Suspended** and **post-pack is stopped** until the shortfall is cleared
7. When you leave, after every epoch is paid, leftover prepaid is returned

```
PSA exists (ops) → fund → start post-pack (leader-time)
    → epoch ends → fee calculated from stake
    → fee taken from prepaid (Rakurai + validator identity → high-priority block reward)
    → if empty: grace, then stream stopped until you top up
    → close → leftover returned
```

> [!CAUTION]
> **An empty PSA stops the stream**
>
> The unpaid streak is tracked in `unpaid_streak` and compared against `grace_epochs` (default **2**). Once it is exceeded the status becomes `Suspended` and post-pack delivery **stops** until the deficit is cleared. Check with [`rakurai-p2c get-account`](../../cli/p2c_subscription.md#31-get-account).

User / consumer steps: [rakurai-p2c](../../cli/p2c_subscription.md). On-chain layout: [PSA struct](#72-psa--p2csubscriptionaccount).

---

## 6. MCA — sharing post-pack backrun profit

### 6.1. Why it exists

**Backrun / MevShare** partners use post-pack without a PSA. [Post-pack](../../../transaction_inclusion/post_pack/README.md) sends leader-time **and** TPU transactions. Profit from backruns sits in **your** wallet.

You **share that profit** with the validator through the MCA (Rakurai takes commission, remainder to the validator).

### 6.2. Working model

1. An **MCA** is created by Rakurai / ops (not a partner CLI path)
2. You keep the key that is allowed to **report** the amount (`record_authority` on the MCA)
3. During the epoch, **nothing** is taken automatically — you trade as usual
4. After the epoch **you report** the shared profit once, then **send that SOL** into the MCA
5. Rakurai’s commission is taken; the **remainder** is paid to the **validator identity** (**block-reward conversion** on by default)

> [!WARNING]
> **Reporting requires the record authority**
>
> Step 4 is signed by the MCA `record_authority` keypair. Without that key you cannot record what you owe, and therefore cannot settle. Confirm which key it is with `rakurai-revshare get-account --detail`.

> [!CAUTION]
> **Settle within about two epochs**
>
> If you do not report and send within roughly **two epochs**, post-pack priority for your service stops.

Partner steps: [rakurai-revshare](../../cli/partner_reward_settlement.md) (`Mev-share`). On-chain layout: [TCA / MCA struct](#7-account-layouts).

---

## 7. Account layouts

Production TCA / MCA are **`RevenueShareAccountV1`** (aliases `TipsCollectionAccountV1` / `MevShareCollectionAccountV1`). PSA is **`P2CSubscriptionAccount`**. Full IDL: [reward_distribution.json](./idl/reward_distribution.json).

### 7.1. TCA / MCA — RevenueShareAccountV1

Same struct for both. `share_kind` is `Tip` (TCA) or `MevShare` (MCA).

**PDA:** `[REVENUE_SHARE_V1, TIP|MEV_SHARE, name[32], vote]`

```rust
pub struct RevenueShareAccountV1 {
    pub share_kind: RevenueKind,           // Tip or MevShare
    pub name: [u8; 32],                    // service id (PDA seed)
    pub validator_vote: Pubkey,
    pub initializer: Pubkey,               // paid rent; gets it back on close
    pub manager_authority: Pubkey,         // claim / config / close
    pub record_authority: Pubkey,          // TCA: validator each leader turn; MCA: partner once post-epoch
    pub max_epoch_entries: u8,
    pub commission_bps: u16,               // Rakurai cut on claim (0 for Rakurai tip TCA)
    pub commission_account: Pubkey,
    pub block_reward_conversion_enabled: bool, // default on
    pub ledger: RevenueLedgerV1,           // Vec<EpochAmountEntryV1>
    pub deficit: u64,                      // unpaid shortfall
    pub bump: u8,
}

pub struct EpochAmountEntryV1 {
    pub epoch: u64,
    pub amount: u64,                       // recorded / attributed
    pub transferred_amount: u64,           // SOL actually settled into the PDA
    pub claimed: bool,
    pub block_reward_converted: bool,
}
```

`pending = amount - transferred_amount`. Inspect: [`rakurai-revshare get-account`](../../cli/partner_reward_settlement.md#31-get-account).

### 7.2. PSA — P2CSubscriptionAccount

**Config PDA:** `[P2C_CONFIG]` → `P2CConfigAccount` (authority + manager / record / max_epoch / commission / grace defaults).

**PSA PDA:** `[P2C_SUBSCRIPTION, name[32], vote]` — anyone may init; fields above are copied from `P2CConfigAccount`.

```rust
pub struct P2CConfigAccount {
    pub authority: Pubkey,                 // update / close this config
    pub manager_authority: Pubkey,         // copied onto each PSA at init
    pub record_authority: Pubkey,          // copied onto each PSA at init
    pub max_epoch_entries: u8,
    pub commission_bps: u16,
    pub commission_account: Pubkey,
    pub grace_epochs: u8,
    pub bump: u8,
}

pub struct P2CSubscriptionAccount {
    pub name: [u8; 32],
    pub validator_vote: Pubkey,
    pub initializer: Pubkey,               // paid rent; residual on close
    pub manager_authority: Pubkey,         // record / claim / config / close
    pub record_authority: Pubkey,          // convert-to-block only (not epoch record)
    pub max_epoch_entries: u8,
    pub commission_bps: u16,
    pub commission_account: Pubkey,
    pub grace_epochs: u8,                  // unpaid epochs before Suspended (default 2)
    pub block_reward_conversion_enabled: bool, // default on
    pub unpaid_streak: u8,
    pub status: P2CSubscriptionStatus,     // Active / InGrace / Suspended
    pub deficit: u64,
    pub ledger: P2CSubscriptionLedger,     // Vec<P2CEpochEntry>
    pub bump: u8,
}

pub struct P2CEpochEntry {
    pub epoch: u64,
    pub stake: u64,                        // snapshot used to price the fee
    pub amount_due: u64,
    pub amount_deducted: u64,              // paid from prepaid on claim
    pub claimed: bool,
    pub block_reward_converted: bool,
}
```

Inspect: [`rakurai-p2c get-account`](../../cli/p2c_subscription.md#31-get-account).

### 7.3. How to view on-chain

You can read the same accounts in an explorer or via CLI.

**CLI (decoded fields)**

```sh
# TCA or MCA
rakurai-revshare -u m -p <RD_PROGRAM_ID> get-account \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VOTE>

# PSA
rakurai-p2c -u m -p <RD_PROGRAM_ID> get-account \
  --name <SERVICE_NAME> -v <VOTE>
```

Use `-u t` and the [testnet program ID](#1-deployed-program-id) on testnet. `get-account` prints the derived PDA — open that address on Solscan.

**Solscan PDA tool**

1. Open [Solscan PDA Create](https://solscan.io/tools#pda-create).
2. Program ID: Reward Distribution ([mainnet](https://solscan.io/account/RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB) / [testnet](https://solscan.io/account/A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB?cluster=testnet)).
3. Seeds:

| Account | Seed 1 (string) | Seed 2 (string) | Seed 3 | Seed 4 |
|---------|-----------------|-----------------|--------|--------|
| TCA | `REVENUE_SHARE_V1` | `TIP` | `name` padded to 32 bytes | vote pubkey |
| MCA | `REVENUE_SHARE_V1` | `MEV_SHARE` | `name` padded to 32 bytes | vote pubkey |
| PSA | `P2C_SUBSCRIPTION` | `name` padded to 32 bytes | vote pubkey | — |

4. Open the derived address on Solscan (add `?cluster=testnet` on testnet) to view lamports and raw data.

---

## 8. How long accounts live

- **RCA** — one per validator per epoch. After about two epochs, leftovers return to the validator and it is closed.
- **TCA / PSA / MCA** — one per service per validator, reused across epochs, until Rakurai closes it. PSA close only after every billed epoch is paid; leftover prepaid is returned.
