# Rakurai Tip Manager Program

**This is the plumbing that turns tips into validator revenue.** Traders tip to get scheduled; this program collects those tips, takes Rakurai's commission, and moves the remainder to the validator automatically on every leader turn, with no action required from either side.

Two design choices are worth knowing. There are **eight tip accounts** rather than one, because a single account would serialize every tipper on the same write lock and cap how many tipped transactions could land in a block. And the drain runs **per leader turn** rather than per epoch, moving tips into the validator's **Tips Collection Account (TCA)** in the [Reward Distribution](../reward_distribution/README.md) program.

➤ IDL: [rakurai_tip_manager.json](./idl/rakurai_tip_manager.json).

---

## 1. Deployed program ID

| Cluster | Tip Manager program |
|---------|---------------------|
| Mainnet | [rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH](https://solscan.io/account/rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH) |
| Testnet | [4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD](https://solscan.io/account/4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD?cluster=testnet) |

The **eight tip account addresses** for each cluster are maintained in one place, on the page tippers actually use: [Tips — tip account addresses](../../../transaction_inclusion/tips.md#appendix-program-and-tip-account-addresses).

---

## 2. How it works

A singleton `TipManagerConfigAccount` stores:

- **`validator_tip_receiver_account`** — current drain destination (the validator’s Rakurai TCA)
- **`client_commission_account` / `client_commission_bps`** — Rakurai cut used on the **next** drain (synced from the TCA that was just claimed)
- **`authority`** — config updater
- **`bumps`** — PDA bumps for the eight tip accounts

`change_tip_receiver_v2` drains using the **current** global commission (set by the previous leader), then copies commission fields from the **new** TCA for the next leader.

> [!NOTE]
> **Commission is one leader turn behind**
>
> The commission applied to a drain is the one the **previous** leader wrote into `TipManagerConfigAccount`, not the one on the TCA being set now. A commission change therefore takes effect on the **next** drain, not the current one. This is intentional: it keeps the split deterministic for tips that were already sitting in the PDAs.

Eight separate tip PDAs exist so many tippers can land at once without serializing on a single write lock.

---

## 3. Account structure

### 3.1. TipManagerConfigAccount

Singleton PDA (`TIP_MANAGER_CONFIG_ACCOUNT`):

| Field | Role |
|-------|------|
| `authority` | Authorized config updater |
| `validator_tip_receiver_account` | Account receiving the validator tip share |
| `client_commission_account` | Rakurai commission destination |
| `client_commission_bps` | Commission in basis points (0–10000) |
| `bumps` | Bumps for the eight tip PDAs |

### 3.2. Rakurai tip accounts

Empty state PDAs that hold SOL. Drain moves all lamports above rent-exempt minimum.

---

## 4. Tip distribution flow

> [!NOTE]
> **Tipping needs no program instruction**
>
> Tippers send a plain `SystemProgram.transfer` to any of the eight PDAs. There is no tip-manager instruction to call and no account to register — the scheduler reads the transfer amount directly. Addresses: [Tips](../../../transaction_inclusion/tips.md#appendix-program-and-tip-account-addresses).

1. **Users send tips** → any of the eight tip accounts (`SystemProgram.transfer`; no tip-manager ix required)
2. **Validator drains** with `change_tip_receiver_v2`:
   - New receiver **must** be a TCA (`REVENUE_SHARE_V1`, `share_kind = Tip`) — the instruction fails against any other account
   - Splits drained SOL: Rakurai commission account vs new TCA
   - CPIs `record_revenue_v1` on the **old** TCA (auto-credits `transferred_amount` for the Rakurai vault)
   - Writes `validator_tip_receiver_account` = new TCA; syncs `client_commission_*` from the new TCA
3. After the epoch, Reward Distribution `claim_revenue_v1` pays the validator identity. The Rakurai-named TCA skips commission at claim (already taken on drain)

Partner **custom tip accounts** are not drained here — they use a per-service TCA and [Partner CLI settlement](../../cli/partner_reward_settlement.md). See [Reward Distribution — TCA](../reward_distribution/README.md#4-tca--tips-for-landing-transactions).

---

## 5. Integration with reward distribution

| | Value |
|--|--------|
| Tip receiver PDA | `[REVENUE_SHARE_V1, TIP, rakurai, vote]` |
| Init | `initialize_revenue_share_account_v1` |
| Drain | `change_tip_receiver_v2` |
| Record CPI | `record_revenue_v1` |

SDK: `derive_rakurai_tip_collection_v1_address`.

How money is split: [Reward Distribution](../reward_distribution/README.md).

---

## 6. Account lifecycle

- **Tip accounts:** stay open; accumulate until the next drain
- **Config account:** stays open until the authority closes it
- Drain never drops a tip PDA below rent exemption
