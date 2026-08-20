# Rakurai Reward Distribution Program

This program is how **four kinds of money** on a Rakurai validator get split and paid out.

You do not need to know how the chain works to follow this. Each kind of money has its own **account** (a labeled wallet the program controls) and its own **flow**. They do not mix.

| | **RCA** | **TCA** | **PSA** | **MCA** |
|--|---------|---------|---------|---------|
| **Full name** | Reward Collection Account | Tips Collection Account | P2C Subscription Account | MevShare Collection Account |
| **What money is this?** | The validator’s **block rewards** | **Tips** traders and landing services pay to land transactions | A **subscription fee** to *use* post-pack, priced from SOL stake | **Backrun profit** from post-pack |
| **Who pays?** | The network (block rewards) | Traders / landing services | Anyone who wants post-pack access | The post-pack user who backran |
| **Why it exists** | Stakers should get their share of block rewards | Tips must be split: Rakurai cut, then the validator | Post-pack is not free; pay a prepaid fee so the stream stays on | Backrun profit must be shared: Rakurai cut, then the validator |
| **During the epoch** | Each time this validator leads, the last block’s reward is split; the **staker share** is parked in the RCA | Tips land in Rakurai tip accounts; Rakurai’s cut is taken; the **rest** is parked in the TCA | You **top up** the PSA so there is prepaid SOL sitting there | Nothing is collected automatically — profit sits with the user |
| **After the epoch** | A payout list is published; **stakers collect** | Remainder → validator (**high-priority block reward**, default) | Fee from prepaid: Rakurai’s cut, rest → validator (**high-priority block reward**, default) | User reports and sends shared profit; rest → validator (**high-priority block reward**, default) |
| **If it is not paid** | Unclaimed staker funds eventually return to the validator | Custom-tip partners who do not settle lose tip priority after a short grace | Stream is **stopped** until the balance is topped up | Users who do not share lose post-pack priority after a short grace |


### Block Reward Conversion for TCA/MCA/PSA Revenue

TCA, PSA, and MCA validator shares, after Rakurai’s commission, are first claimed and credited to the validator identity.
With **block-reward conversion enabled by default**, the claimed amount is converted into a high-priority block-reward transaction during the validator’s leader turn. The transaction must land within that leader turn; otherwise, it is **dropped and not forwarded to the next leader**.
**Post-pack users:** fund the **PSA** first, then share backrun profit (**MCA**).

Where to click: [P2C subscription](../../cli/p2c_subscription.md) (PSA) · [Partner settlement](../../cli/partner_reward_settlement.md) (TCA / MCA) · [Account layouts](#6-account-layouts) · [View on Solscan](#63-how-to-view-on-chain).

➤ On-chain interface file: [reward_distribution.json](./idl/reward_distribution.json).

---

## 1. Deployed program ID

- **Mainnet**: [RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB](https://solscan.io/account/RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB)
- **Testnet**: [A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB](https://solscan.io/account/A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB?cluster=testnet)

---

## 2. RCA — block rewards for stakers

### Why it exists

When a validator produces blocks, Solana pays **block rewards**. Those rewards belong in part to **people who staked** with that validator, not only to the operator.

The RCA is the holding account for the **stakers’ share** for one validator, for one epoch (~2 days).

### 2.1 Epoch flow

### 2.1.1 RewardCollectionAccount account initialization
On the first turn of each epoch, the `RewardCollectionAccount` is automatically initialized by the Rakurai Solana client. This initialization includes:
- Commission details (from validator-specific [`RakuraiActivationAccount`](../rakurai_activation/README.md#4-rakurai-activation-account-creation)).
- Authority to update the reward Merkle root (only this authority can upload the Merkle root to the `RewardCollectionAccount` account).
> Account initialization logic is part of the Rakurai Solana client.


### 2.1.2. Per-turn transfers
During every leader turn:
- The **previous turn's block reward** is processed:
  - **Client commission** → transferred to client (i.e., Rakurai) account.
  - **Validator commission** → remains in the validator's identity account.
  - **Staker share** → accumulated into the `RewardCollectionAccount`.

> Because the reward of the current turn is transferred during the next one, the **first turn** of an epoch handles the **last reward** of the previous epoch.

---

### 2.1.3. Post-epoch staker distribution
At the final slot of each epoch, the following process takes place:
- A snapshot of Solana accounts is captured.
- Each validator's staker details and stake weights are extracted.
- An off-chain Merkle tree is generated containing reward share data.
  - **Extra flexibility**: At this stage, specific stakers can be blacklisted, and individual stake weights can be adjusted before finalizing the tree.
- The Merkle root is uploaded to the `RewardCollectionAccount` by the `reward_merkle_root_authority`.
- Stakers receive rewards via Merkle claims. When `reward_merkle_root_authority` is Rakurai, Rakurai runs the claim process on behalf of stakers.


## 2.2. Reward Distribution — Free and Automated by Rakurai

- Set the Merkle root authority to [Rakurai](../../../validators/setup_and_build.md#5-add-additional-cli-args) for fully automated reward distribution.
- Keep it yourself if you want to run distribution manually.

When set to **Rakurai**, Rakurai will automatically:

1. **Create a snapshot**
2. **Calculate the Merkle root**
3. **Upload it on-chain**
4. **Run the claim process for stakers**

<p style="font-size:14px;">
    <span style="color:#66ff66;"><i><b>0% distribution fees charged by Rakurai</b></i></span>
    — only standard Solana transaction fees apply.
</p>

---

### 2.3. Client commission on MEV rewards

The client charges commission on MEV Rewards **only** if the following conditions are met:

- The validator is actively running **Rakurai during that epoch**.  
- The validator has set a non-zero **MEV commission** in their **Tip Distribution Account**.

*Note:* If the validator’s MEV commission is **0%**, Rakurai does **not** charge any commission on MEV tips.

---

### 2.3.1. Deduction flow

1. The validator’s share of MEV tips is credited to their **vote account** by the Tip Distribution Program in the following epoch.  
2. A `ClaimStatus` account is created to track that the validator has received MEV rewards.  
3. Rakurai client monitors the `ClaimStatus` account; once it is created, then it is eligible to deduct commission.  
4. Rakurai cannot deduct directly from the vote account, so the **same commission amount** is deducted from the validator’s **identity account** instead.  
5. The deduction is performed by invoking the `transfer_client_commission_on_mev_commission` instruction in the **Reward Distribution Program**.  
6. The commission rate is defined in the **Reward Distribution Config account**.

---

## 3. TCA — tips for landing transactions

### Why it exists

Traders and transaction-landing services pay a **tip** so the Rakurai scheduler will prioritize their transactions. Those tips must be split: **Rakurai gets a commission**, the **validator gets the rest**.

The TCA is where the **validator’s tip remainder** is collected for the epoch, then paid to the validator after the epoch ends (typically claimed in the next epoch).

### Working model — Rakurai tip accounts (usual case)

By default, services tip Rakurai's [eight tip accounts](../rakurai_tip_manager/README.md), and `rakurai_tip_manager` drains them automatically. But an external transaction-landing service can [register their own custom tip account](../../../transaction_inclusion/rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts) and agree to share a commission (e.g. 30%) with Rakurai.

1. A trader tips **any of the eight accounts**
2. Each time this validator is leader, those tip accounts are emptied
3. **Rakurai’s commission** is taken immediately
4. The **remainder** is moved into this validator’s TCA
5. After the epoch, that remainder is paid to the **validator’s identity**, then converted to a **high-priority block reward** (on by default)

Rakurai is not paid a second time at step 5 — the commission already happened at step 3.

### Working model — custom tip account (partner)

Some landing services want tips in **their own** account. Rakurai cannot empty that account.

1. You register the account and an agreed share with Rakurai ([FAQ](../../../transaction_inclusion/rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts))
2. During the epoch, the validator **writes down** what is owed (no SOL moves yet)
3. After the epoch, **you send** the owed SOL into the TCA
4. Then Rakurai’s commission is taken from what you sent, and the rest goes to the validator identity (same **block-reward conversion** as above)

If you do not settle within about **two epochs**, that custom tip account stops being used for priority.

Partner steps: [rakurai-revshare](../../cli/partner_reward_settlement.md) (`Tip`). On-chain layout: [TCA / MCA struct](#6-account-layouts).

---

## 4. PSA — prepaid fee to use post-pack

### Why it exists

Anyone who wants **P2C / post-pack** must pay a **subscription** to receive the stream. This is not a tip and not a share of backrun profit — it is the **price of access**, based on SOL stake (a public number you can check on explorers).

From each epoch’s fee: **commission to Rakurai**, **remainder to the validator**.

Which servers receive the stream is configured separately in [Client Config](../rakurai_client_config/README.md) (always submit the **full** current list plus any new endpoint). The PSA only holds the **prepaid SOL**.

Full product guide: [Post-pack confirmations](../../../transaction_inclusion/post_pack_confirmations.md).

### Working model

1. Rakurai opens a PSA for your service + this validator
2. **You top up** SOL into that account (any wallet can fund it)
3. Epoch ends. Rakurai writes the stake snapshot and the fee due
4. The fee is taken from prepaid: Rakurai’s cut, rest to the validator identity (**block-reward conversion** on by default)
5. If the balance is too low, top up and try again — or the shortfall is booked as **deficit**
6. After a short grace, status becomes **Suspended** and **post-pack is stopped** until you clear the deficit
7. When you leave, after every epoch is paid, leftover prepaid is returned

```
open account
    → you top up
    → epoch ends → fee calculated from stake
    → fee taken from prepaid (Rakurai + validator identity → high-priority block reward)
    → if empty: grace, then stream stopped until you top up
    → close → leftover returned
```

User / consumer steps: [rakurai-p2c](../../cli/p2c_subscription.md). On-chain layout: [PSA struct](#62-psa--p2csubscriptionaccount).

---

## 5. MCA — sharing post-pack backrun profit

### Why it exists

After you fund the **PSA**, [post-pack](../../../transaction_inclusion/post_pack_confirmations.md) sends you transactions at the **point of no return** (too late for anyone to front-run). You can **backrun** (trade after them). That extra profit sits in **your** wallet.

The deal is: you **share that profit** with the validator. The MCA is the account that receives the shared amount so Rakurai can take commission and pay the validator.

### Working model

1. When you enable post-pack, Rakurai opens an MCA for **your service + this validator**
2. You keep the key that is allowed to **report** the amount (without it you cannot update the books)
3. During the epoch, **nothing** is taken automatically — you trade as usual
4. After the epoch **you report** the shared profit once, then **send that SOL** into the MCA
5. Rakurai’s commission is taken; the **remainder** is paid to the **validator identity** (**block-reward conversion** on by default)

If you do not report and send within about **two epochs**, post-pack priority for your service stops.

Partner steps: [rakurai-revshare](../../cli/partner_reward_settlement.md) (`Mev-share`). On-chain layout: [TCA / MCA struct](#6-account-layouts).

---

## 6. Account layouts

Production TCA / MCA are **`RevenueShareAccountV1`** (aliases `TipsCollectionAccountV1` / `MevShareCollectionAccountV1`). PSA is **`P2CSubscriptionAccount`**. Full IDL: [reward_distribution.json](./idl/reward_distribution.json).

### 6.1. TCA / MCA — RevenueShareAccountV1

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

### 6.2. PSA — P2CSubscriptionAccount

**PDA:** `[P2C_SUBSCRIPTION, name[32], vote]`

```rust
pub struct P2CSubscriptionAccount {
    pub name: [u8; 32],
    pub validator_vote: Pubkey,
    pub initializer: Pubkey,
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

Inspect: [`rakurai-p2c get-account`](../../cli/p2c_subscription.md#5-examples).

### 6.3. How to view on-chain

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

## 7. How long accounts live

- **RCA** — one per validator per epoch. After about two epochs, leftovers return to the validator and it is closed.
- **TCA / PSA / MCA** — one per service per validator, reused across epochs, until Rakurai closes it. PSA close only after every billed epoch is paid; leftover prepaid is returned.
