# TIN — Transaction Inclusion Network

Documentation for searchers, block engines, and partners integrating with Rakurai validators.

**Audience:** TIN partners, MEV searchers, and traders sending bundles or consuming post-pack confirmations.

---

## 1. Guides

| Guide | Description |
|-------|-------------|
| [Transaction inclusion network](./transaction_inclusion.md) | Bundles, virtual priority boost, and post-pack confirmations |
| [Post-pack confirmations](./post_pack_confirmations.md) | What post-pack, **PSA**, and **MCA** are; how to use P2C (packets and bundles); then commands |
| [Tips FAQ](./rakurai_tip_manager_faqs.md) | Sending tips, prioritization, custom accounts, and distribution |
| [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md) | Top up **PSA** prepaid subscription (`rakurai-p2c`) |
| [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) | Settle **TCA** / **MCA** (`rakurai-revshare`) |

---

## 2. Validator operations

For validator-side configuration (Admin RPC, Geyser, binary verification), see [Validators](../validators/README.md).

---

## 3. Note: do not double-count tips and converted block rewards

External indexers that watch **transfers into Rakurai tip accounts** and also watch **validator block rewards** can count the **same SOL twice**.

What happens on-chain:

1. A **tip** (TCA), **PSA** fee, or **MCA** share is claimed to the validator identity.
2. If **`block_reward_conversion_enabled`** is set on that **TCA / PSA / MCA** account (this flag is **on by default**), the claimed amount is sent again as a **high-priority block-reward** transaction during a leader turn.

That block-reward transaction is the **converted claim**, not new revenue. If you already counted the tip (or the PSA/MCA payout), counting the later block reward as extra income is double counting.

**How to avoid it**

- Read `block_reward_conversion_enabled` on the **TCA**, **PSA**, and **MCA** accounts (see [Reward Distribution — block-reward conversion](../rakurai_programs/programs/reward_distribution/README.md#block-reward-conversion-for-tcamcapsa-revenue)).
- If the flag is **on**, count the money **once**: either at the tip / claim, or as the converted block reward — not both.

The conversion transaction must land in **that leader turn** or it is dropped (it is not forwarded to the next leader).
